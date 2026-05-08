#!/usr/bin/env python3
"""Audit + fix data consistency for case e50f01e8.

After the workflow seed and the mock seed there are still gaps the report
surfaces: LoanCase.case_id null, LoanCase.scoring_result_id/ai_recommendation_id
null even though linked rows exist, ApplicantDossier.dossier_id / loan_case_id
null. Aligns those so the report renders coherently.
"""

import json
import urllib.request

LOCAL = "http://localhost:7001"
CASE = "e50f01e8-eebd-4da6-839f-7e916a7f766f"
APP = "APP-ORCH-003"
DOSSIER_VALUE = "8f8e40b4-6e6a-4efb-8d16-c4c57ebf2a5a"


def req(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json"} if body is not None else {}
    r = urllib.request.Request(LOCAL + path, data=data, method=method, headers=headers)
    with urllib.request.urlopen(r, timeout=60) as resp:
        return json.loads(resp.read() or b"null")


def filter_one(schema_name, field, value):
    out = req(
        "POST",
        f"/api/runtime/object-model/instances/schema/{schema_name}/filter",
        {"condition": {"op": "EQ", "arguments": [field, value]}, "limit": 1},
    )
    rows = out.get("instances", [])
    return rows[0] if rows else None


sm = {s["name"]: s["id"] for s in req("GET", "/api/runtime/object-model/schemas?limit=500")["schemas"]}

# Find the dossier row for this application
dossier_row = filter_one("ApplicantDossier", "loan_application_id", APP)
if not dossier_row:
    raise SystemExit(f"No dossier for {APP}")
dossier_row_id = dossier_row["id"]
print(f"Dossier row: {dossier_row_id}")

# Find the L2/L3 rows for this dossier (by the value shared across them)
sr = filter_one("ScoringResult", "dossier_id", DOSSIER_VALUE)
ai = filter_one("AIRecommendation", "dossier_id", DOSSIER_VALUE)
if not (sr and ai):
    raise SystemExit("missing ScoringResult/AIRecommendation for this dossier")
print(f"ScoringResult row: {sr['id']}")
print(f"AIRecommendation row: {ai['id']}")

# 1. Pin missing fields on the dossier row
req(
    "PUT",
    f"/api/runtime/object-model/instances/{sm['ApplicantDossier']}/{dossier_row_id}",
    {"properties": {
        "dossier_id": DOSSIER_VALUE,
        "loan_case_id": CASE,
    }},
)
print(f"~ ApplicantDossier {dossier_row_id}: pinned dossier_id + loan_case_id")

# 2. Pin missing pointers on LoanCase
req(
    "PUT",
    f"/api/runtime/object-model/instances/{sm['LoanCase']}/{CASE}",
    {"properties": {
        "case_id": "CASE-ORCH-003",
        "scoring_result_id": sr["id"],
        "ai_recommendation_id": ai["id"],
    }},
)
print(f"~ LoanCase {CASE}: pinned case_id, scoring_result_id, ai_recommendation_id")

# 3. Pin LoanCase pointers onto the AIRecommendation/ScoringResult rows (loan_case_id is currently a stale value)
req(
    "PUT",
    f"/api/runtime/object-model/instances/{sm['ScoringResult']}/{sr['id']}",
    {"properties": {"loan_case_id": CASE}},
)
print(f"~ ScoringResult {sr['id']}: loan_case_id -> {CASE}")
req(
    "PUT",
    f"/api/runtime/object-model/instances/{sm['AIRecommendation']}/{ai['id']}",
    {"properties": {"loan_case_id": CASE}},
)
print(f"~ AIRecommendation {ai['id']}: loan_case_id -> {CASE}")


# 4a. Make sure the trace contains a real hard-fail breach so the audit page
# demonstrates the severity + outcome distinction. Flip one specific
# affordability hard-fail rule from pass→fail and let the rest of the script
# pick it up via aggregation.
HARD_FAIL_DEMO_RULE = "RG209-AFF-002"
ro_target = req(
    "POST",
    "/api/runtime/object-model/instances/schema/RuleOutcome/filter",
    {"condition": {"op": "AND", "arguments": [
        {"op": "EQ", "arguments": ["dossier_id", DOSSIER_VALUE]},
        {"op": "EQ", "arguments": ["rule_id", HARD_FAIL_DEMO_RULE]},
    ]}, "limit": 1},
)["instances"]
if ro_target:
    target_row = ro_target[0]
    if target_row["properties"].get("outcome") != "fail":
        req(
            "PUT",
            f"/api/runtime/object-model/instances/{sm['RuleOutcome']}/{target_row['id']}",
            {"properties": {
                "outcome": "fail",
                "applied": True,
                "contribution": -40.0,
                "message": (
                    f"{HARD_FAIL_DEMO_RULE} hard-fail: stressed monthly surplus "
                    f"falls below required affordability buffer at the requested "
                    f"loan size."
                ),
            }},
        )
        print(f"~ RuleOutcome {target_row['id']}: {HARD_FAIL_DEMO_RULE} flipped to fail")
    else:
        print(f"= RuleOutcome {HARD_FAIL_DEMO_RULE}: already failing")
else:
    print(f"! Could not find {HARD_FAIL_DEMO_RULE} in trace; skipping demo flip")

# 4b. Reconcile ScoringResult totals with the actual RuleOutcome trace.
ro_rows = req(
    "POST",
    "/api/runtime/object-model/instances/schema/RuleOutcome/filter",
    {"condition": {"op": "EQ", "arguments": ["dossier_id", DOSSIER_VALUE]}, "limit": 500},
)["instances"]

hard_fail_breaches = []
flag_breaches = []
penalty_breaches = []
active_rule_ids = []
for r in ro_rows:
    p = r["properties"]
    rid = p.get("rule_id")
    if rid and rid not in active_rule_ids:
        active_rule_ids.append(rid)
    if p.get("outcome") != "fail":
        continue
    summary = {
        "rule_id": rid,
        "severity": p.get("severity"),
        "category": p.get("category"),
        "contribution": p.get("contribution"),
        "message": p.get("message"),
    }
    sev = p.get("severity")
    if sev == "hard_fail":
        hard_fail_breaches.append(summary)
    elif sev == "flag":
        flag_breaches.append(summary)
    elif sev == "score_penalty":
        penalty_breaches.append(summary)

# Weighted score (0–100). Start at 100, subtract aggregated penalties + 25 per
# flag breach, floor at 0. Hard fails make it 0.
score = 100
for b in penalty_breaches:
    try:
        score -= int(round(float(b.get("contribution") or 0)))
    except (TypeError, ValueError):
        pass
score -= 25 * len(flag_breaches)
if hard_fail_breaches:
    score = 0
score = max(0, min(100, score))

decision_band = (
    "DECLINED" if hard_fail_breaches
    else ("CONSIDER" if flag_breaches or penalty_breaches else "APPROVED")
)
reason_summary = (
    f"{len(hard_fail_breaches)} hard-fail breach(es), "
    f"{len(flag_breaches)} flag breach(es), "
    f"{len(penalty_breaches)} score-penalty breach(es). "
    f"Weighted score {score}/100 → decision_band {decision_band}."
)

req(
    "PUT",
    f"/api/runtime/object-model/instances/{sm['ScoringResult']}/{sr['id']}",
    {"properties": {
        "hard_fails": hard_fail_breaches,
        "flags": flag_breaches,
        "weighted_score": score,
        "decision_band": decision_band,
        "decision_reason_summary": reason_summary,
        "passes_serviceability": True,
        "tmd_within_target_market": True,
        "active_rule_ids": active_rule_ids,
        "rule_pack_version": "draft-corpus-2026-05-05",
        "scoring_method_version": "score-2025-07",
    }},
)
print(
    f"~ ScoringResult {sr['id']}: hard_fails={len(hard_fail_breaches)}, "
    f"flags={len(flag_breaches)}, score={score}, band={decision_band}"
)


# 5. Re-narrate the AI recommendation around the same numbers.
flag_rule_ids = [b["rule_id"] for b in flag_breaches]
hard_fail_rule_ids = [b["rule_id"] for b in hard_fail_breaches]

ai_decision = "DECLINED" if hard_fail_breaches else (
    "CONSIDER" if flag_breaches else "APPROVED"
)
ai_confidence = 0.82 if hard_fail_breaches else (0.78 if flag_breaches else 0.9)

ai_reasons = []
if hard_fail_breaches:
    ai_reasons.append({
        "label": f"Hard-fail breach on {hard_fail_rule_ids[0]}",
        "supports": "decline",
        "citations": [{"rule_id": rid} for rid in hard_fail_rule_ids],
        "summary": hard_fail_breaches[0]["message"],
    })
else:
    ai_reasons.append({
        "label": "All hard-fail rules cleared",
        "supports": "consider",
        "citations": [{"rule_id": rid} for rid in active_rule_ids[:3]],
        "summary": f"{len(active_rule_ids)} rules ran; 0 hard-fail breaches.",
    })
if flag_breaches:
    ai_reasons.append({
        "label": "Flag breach",
        "supports": "consider",
        "citations": [{"rule_id": rid} for rid in flag_rule_ids],
        "summary": flag_breaches[0]["message"],
    })
ai_reasons.append({
    "label": "Serviceability stress buffer",
    "supports": "consider",
    "citations": [{"dossier_field": "financial_summary.monthly_surplus_aud"}],
    "summary": (
        "Stressed surplus is positive at the standard 20% buffer but the "
        "affordability rule applies a tighter threshold; manual review needed "
        "to confirm headroom."
    ),
})

ai_risk_flags = [
    {"rule_id": b["rule_id"], "category": b["category"], "severity": b["severity"]}
    for b in (hard_fail_breaches + flag_breaches)
]
ai_actions = []
if hard_fail_breaches:
    ai_actions.append({"action": "decline_or_escalate_to_senior_credit", "owner": "credit_officer"})
    ai_actions.append({"action": "verify_essential_outgoings_evidence", "owner": "credit_officer"})
if flag_breaches:
    ai_actions.append({"action": "request_3mo_bank_statement", "owner": "applicant"})

primary_concern = (
    hard_fail_rule_ids[0] if hard_fail_rule_ids
    else (flag_rule_ids[0] if flag_rule_ids else "n/a")
)
ai_draft_email = (
    "Hi Avery,\n\n"
    "Thanks for your application. Our automated assessment identified an "
    f"affordability concern under {primary_concern} (stressed monthly surplus "
    "vs requested repayment). Before we can proceed we'll need to verify a "
    "few items in writing:\n\n"
    "  • Most recent 3-month bank statement\n"
    "  • Evidence of any liquid assets you'd draw on if income paused\n"
    "  • Confirmation of any upcoming changes in regular outgoings\n\n"
    "We'll get back to you within 2 business days of receiving these.\n\n"
    "Regards,\nBlockEarner Credit Team\n"
)

req(
    "PUT",
    f"/api/runtime/object-model/instances/{sm['AIRecommendation']}/{ai['id']}",
    {"properties": {
        "decision": ai_decision,
        "confidence_score": ai_confidence,
        "citation_check_passed": True,
        "reasons": ai_reasons,
        "risk_flags": ai_risk_flags,
        "recommended_actions": ai_actions,
        "draft_email": ai_draft_email,
        "scoring_result_id": sr["id"],
        "dossier_id": DOSSIER_VALUE,
    }},
)
print(f"~ AIRecommendation {ai['id']}: decision={ai_decision}, "
      f"reasons={len(ai_reasons)}, risk_flags={len(ai_risk_flags)}")


# 6. Tighten the human override reasoning to reference the actual breaches.
hd = filter_one("HumanDecision", "loan_case_id", CASE)
hd_id = hd["id"]

if hard_fail_breaches:
    primary_rule = hard_fail_rule_ids[0]
    override_reasoning = (
        f"Scoring returned DECLINED on {primary_rule} hard-fail (stressed "
        f"surplus vs requested repayment). AI also recommended DECLINED "
        f"({ai_decision}, conf {ai_confidence:.2f}). Manual verification: "
        f"applicant produced a 3-month bank statement showing AUD 20k+ "
        f"liquid buffer, employer letter confirming 36mo tenure and base "
        f"salary, plus written confirmation that the upcoming end-of-year "
        f"bonus is unrelated to the repayment plan. Recomputed stressed "
        f"surplus including liquid buffer and conservative bonus exclusion "
        f"clears the affordability threshold. Approving at the requested "
        f"limit with mandatory 6-month file review and pinned LVR check."
    )
    overrode_ai = True
    overrode_scoring = True
elif flag_breaches:
    primary_rule = flag_rule_ids[0]
    override_reasoning = (
        f"AI returned CONSIDER on a single flag breach ({primary_rule}) "
        f"around essential outgoings vs repayment capacity. Verified payslip "
        f"+ 3-month bank statement; applicant has 36mo employment tenure, "
        f"~AUD 20k liquid buffer (>3mo expenses) and stable income "
        f"reliability. All hard-fail rules cleared. Approving at requested "
        f"limit; pin file for 6-month review."
    )
    overrode_ai = True
    overrode_scoring = False
else:
    override_reasoning = (
        "Scoring and AI both APPROVED. Standard credit-officer sign-off."
    )
    overrode_ai = False
    overrode_scoring = False

req(
    "PUT",
    f"/api/runtime/object-model/instances/{sm['HumanDecision']}/{hd_id}",
    {"properties": {
        "decision": "APPROVED",
        "overrode_ai": overrode_ai,
        "overrode_scoring": overrode_scoring,
        "override_reasoning": override_reasoning,
        "ai_recommendation_id": ai["id"],
        "scoring_result_id": sr["id"],
    }},
)
print(
    f"~ HumanDecision {hd_id}: override APPROVED "
    f"(overrode_scoring={overrode_scoring}, overrode_ai={overrode_ai})"
)


# 7. Re-write LoanCase.decision_events with totals that match ScoringResult.
human_summary = "Credit officer approves"
if overrode_scoring and overrode_ai:
    human_summary += ", overriding scoring DECLINED + AI DECLINED on verified buffer evidence"
elif overrode_ai:
    human_summary += f", overriding AI {ai_decision}"
decision_events = [
    {"seq": 1, "timestamp": "2026-05-04T09:45:00+00:00", "layer": "L1",
     "actor": "system", "event_type": "dossier_assembled",
     "summary": "Layer 1 data collection complete: Experian + Merkle + serviceability"},
    {"seq": 2, "timestamp": "2026-05-05T18:00:00+00:00", "layer": "L2",
     "actor": "system", "event_type": "scoring_complete",
     "summary": (
         f"Deterministic scoring: {decision_band}, weighted score {score}/100, "
         f"{len(hard_fail_breaches)} hard-fail breach(es), "
         f"{len(flag_breaches)} flag breach(es)."
     )},
    {"seq": 3, "timestamp": "2026-05-05T20:30:00+00:00", "layer": "L3",
     "actor": "agent:layer3-ai-reviewer", "event_type": "ai_review_complete",
     "summary": (
         f"AI recommends {ai_decision} at confidence {ai_confidence:.2f}, "
         f"citations OK"
     )},
    {"seq": 4, "timestamp": "2026-05-05T22:15:00+00:00", "layer": "L4",
     "actor": "volodymyr@agilevision.io", "event_type": "human_decision",
     "summary": human_summary},
]
req(
    "PUT",
    f"/api/runtime/object-model/instances/{sm['LoanCase']}/{CASE}",
    {"properties": {"decision_events": decision_events}},
)
print(f"~ LoanCase {CASE}: decision_events re-narrated to match scoring totals")
