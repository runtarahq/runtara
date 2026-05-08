#!/usr/bin/env python3
"""Seed mock data so case e50f01e8…/APP-ORCH-003 renders every audit block.

Backfills:
- LoanApplication APP-ORCH-003 (none existed)
- ServiceabilityCalc for APP-ORCH-003 (none existed)
- HumanDecision (capture id, point LoanCase.human_decision_id at it)
- TMDDistributionEvent for the case (capture id, point LoanCase.tmd_event_id at it)

Idempotent: existing rows for the same keys are reused instead of duplicated.
"""

import json
import os
import sys
import urllib.error
import urllib.request

LOCAL = os.environ.get("LOCAL_URL", "http://localhost:7001")
CASE_ID = "e50f01e8-eebd-4da6-839f-7e916a7f766f"
APP_ID = "APP-ORCH-003"
DOSSIER_ID = "8f8e40b4-6e6a-4efb-8d16-c4c57ebf2a5a"


def http(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json"} if body is not None else {}
    req = urllib.request.Request(LOCAL + path, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"null")


def schemas():
    s, d = http("GET", "/api/runtime/object-model/schemas?limit=500")
    if s != 200:
        raise SystemExit(f"list schemas failed: {s} {d}")
    return {row["name"]: row["id"] for row in d["schemas"]}


def filter_one(schema_name, field, value):
    s, d = http(
        "POST",
        f"/api/runtime/object-model/instances/schema/{schema_name}/filter",
        {"condition": {"op": "EQ", "arguments": [field, value]}, "limit": 1},
    )
    if s != 200:
        raise SystemExit(f"filter {schema_name} failed: {s} {d}")
    rows = d.get("instances", [])
    return rows[0] if rows else None


def create_instance(schema_id, properties):
    s, d = http(
        "POST",
        f"/api/runtime/object-model/instances/{schema_id}/bulk",
        {"instances": [properties], "onError": "stop"},
    )
    if s not in (200, 201):
        raise SystemExit(f"create failed: {s} {d}")
    # bulk_create doesn't return ids, fall back to filter by a unique field
    return d


def patch_instance(schema_id, instance_id, properties):
    s, d = http(
        "PUT",
        f"/api/runtime/object-model/instances/{schema_id}/{instance_id}",
        {"properties": properties},
    )
    if s not in (200, 201):
        raise SystemExit(f"update failed: {s} {d}")
    return d


def ensure(name, schema_id, key_field, key_value, payload):
    """Create the row if no row exists with key_field=key_value; return its id."""
    existing = filter_one(name, key_field, key_value)
    if existing:
        print(f"= {name}: existing row {existing['id']}")
        return existing["id"]
    create_instance(schema_id, payload)
    fresh = filter_one(name, key_field, key_value)
    if not fresh:
        raise SystemExit(f"created {name} but cannot re-read it back")
    print(f"+ {name}: created {fresh['id']}")
    return fresh["id"]


def main():
    sm = schemas()

    # 1) LoanApplication APP-ORCH-003
    ensure("LoanApplication", sm["LoanApplication"], "application_id", APP_ID, {
        "application_id": APP_ID,
        "submitted_at": "2026-05-04T09:30:00+00:00",
        "channel": "web",
        "status": "decided",
        "applicant": {
            "full_name": "Avery Ng",
            "dob": "1992-11-04",
            "residency_status": "citizen",
            "address": {"line1": "12 Macquarie St", "city": "Sydney", "state": "NSW", "postcode": "2000"},
            "contact": {"email": "avery.ng@example.com", "phone": "+61 400 123 456"},
        },
        "employment": {
            "status": "employed_full_time",
            "employer": "Acme Pty Ltd",
            "role": "Senior Engineer",
            "tenure_months": 36,
            "annual_gross_income_aud": 110000,
        },
        "financial_position": {
            "monthly_net_income_aud": 6900,
            "monthly_expenses_aud": 4200,
            "existing_debt_monthly_aud": 0,
            "liquid_assets_aud": 20000,
        },
        "requested_loan": {
            "product_id": "BE-PERSONAL-LOAN-v2025-07",
            "loan_amount_aud": 60000,
            "term_months": 18,
            "purpose": "home_improvement",
        },
        "collateral": {
            "asset": "BTC",
            "pledged_amount": 1.2,
            "wallet_address": "bc1q8z3...mock",
            "wallet_chain": "bitcoin",
        },
        "consent": {"credit_check": True, "data_sharing": True, "comprehensive_credit_reporting": True},
        "self_attested_flags": {
            "meets_objectives": True,
            "meets_requirements": True,
            "understands_obligations": True,
            "has_repayment_plan": True,
        },
        "uploaded_documents": [
            {"kind": "payslip", "filename": "payslip_apr2026.pdf"},
            {"kind": "bank_statement", "filename": "bank_3mo.pdf"},
        ],
    })

    # 2) ServiceabilityCalc for APP-ORCH-003
    ensure("ServiceabilityCalc", sm["ServiceabilityCalc"], "loan_application_id", APP_ID, {
        "calc_id": "SVC-MOCK-ORCH-003",
        "loan_application_id": APP_ID,
        "calculated_at": "2026-05-04T09:45:00+00:00",
        "household_size": "single",
        "num_dependents": 0,
        "location_tier": "capital_city",
        "annual_gross_income_aud": 110000,
        "monthly_net_income_aud": 6900,
        "declared_monthly_expenses_aud": 4200,
        "hem_benchmark_id": "HEM-2026-01-CAP-SINGLE",
        "hem_monthly_amount_aud": 2200,
        "applied_expenses_aud": 4200,
        "existing_debt_monthly_aud": 0,
        "loan_repayment_modelled_aud": 3697.5,
        "monthly_surplus_aud": 1002.5,
        "stress_buffer_pct": 0.20,
        "monthly_surplus_stressed_aud": 263.0,
        "passes_serviceability": True,
        "calc_inputs_snapshot": {
            "income_source": "payslip",
            "expense_source": "declared+hem_floor",
            "stress_method": "rate+0.02",
        },
        "calc_method_version": "svc-2025-07",
    })

    # 3) HumanDecision — create then point LoanCase.human_decision_id at it
    hd = filter_one("HumanDecision", "loan_case_id", CASE_ID)
    if hd:
        hd_id = hd["id"]
        print(f"= HumanDecision: existing row {hd_id}")
    else:
        create_instance(sm["HumanDecision"], {
            "human_decision_id": "HD-MOCK-ORCH-003",
            "loan_case_id": CASE_ID,
            "ai_recommendation_id": "ai-rec-mock-orch-003",
            "scoring_result_id": "scoring-mock-orch-003",
            "decided_at": "2026-05-05T22:15:00+00:00",
            "reviewer_id": "volodymyr@agilevision.io",
            "reviewer_role": "credit_officer",
            "decision": "APPROVED",
            "overrode_ai": True,
            "overrode_scoring": False,
            "override_reasoning": (
                "AI flagged CONSIDER on borderline DTI but applicant has 36mo stable tenure, "
                "additional liquid buffer (>3mo expenses), and clear repayment plan. "
                "Approve at the requested limit; pin review at 6mo."
            ),
            "reviewer_notes": "Verified payslip + 3mo bank stmt. Crypto custody confirmed.",
            "info_requested": [],
            "signal_response_id": "sig-mock-orch-003",
            "report_snapshot_ref": f"snapshot/{CASE_ID}/2026-05-05",
        })
        hd = filter_one("HumanDecision", "loan_case_id", CASE_ID)
        hd_id = hd["id"]
        print(f"+ HumanDecision: created {hd_id}")

    # 4) TMDDistributionEvent for the case
    tmd = filter_one("TMDDistributionEvent", "loan_case_id", CASE_ID)
    if tmd:
        tmd_id = tmd["id"]
        print(f"= TMDDistributionEvent: existing row {tmd_id}")
    else:
        create_instance(sm["TMDDistributionEvent"], {
            "tmd_event_id": "TMD-MOCK-ORCH-003",
            "loan_case_id": CASE_ID,
            "loan_application_id": APP_ID,
            "assessed_at": "2026-05-05T22:20:00+00:00",
            "tmd_version": "2025-07-07",
            "within_target_market": True,
            "eligibility_outcomes": [
                {"input": "is_18_or_older", "value": True, "passes": True},
                {"input": "residency_ok", "value": True, "passes": True},
                {"input": "employment_ok", "value": True, "passes": True},
                {"input": "has_au_bank_account", "value": True, "passes": True},
                {"input": "collateral_ok", "value": True, "passes": True},
            ],
            "negative_market_flags": [],
            "is_significant_dealing": False,
            "significant_dealing_rationale": (
                "All TMD eligibility inputs pass with no negative-market flags. "
                "Distribution falls within target market; no significant dealing event."
            ),
            "final_decision_outcome": "APPROVED",
            "reported_to_compliance_at": None,
        })
        tmd = filter_one("TMDDistributionEvent", "loan_case_id", CASE_ID)
        tmd_id = tmd["id"]
        print(f"+ TMDDistributionEvent: created {tmd_id}")

    # 5) Update LoanCase to point at the (possibly new) IDs
    patch_instance(sm["LoanCase"], CASE_ID, {
        "human_decision_id": hd_id,
        "tmd_event_id": tmd_id,
        # close the case with a couple of decision events
        "closed_at": "2026-05-05T22:25:00+00:00",
        "decision_events": [
            {"seq": 1, "timestamp": "2026-05-04T09:45:00+00:00", "layer": "L1",
             "actor": "system", "event_type": "dossier_assembled",
             "summary": "Layer 1 data collection complete: Experian + Merkle + serviceability"},
            {"seq": 2, "timestamp": "2026-05-05T18:00:00+00:00", "layer": "L2",
             "actor": "system", "event_type": "scoring_complete",
             "summary": "Deterministic scoring: CONSIDER, weighted score 64, 0 hard fails"},
            {"seq": 3, "timestamp": "2026-05-05T20:30:00+00:00", "layer": "L3",
             "actor": "agent:layer3-ai-reviewer", "event_type": "ai_review_complete",
             "summary": "AI recommends CONSIDER at confidence 0.78, citations OK"},
            {"seq": 4, "timestamp": "2026-05-05T22:15:00+00:00", "layer": "L4",
             "actor": "volodymyr@agilevision.io", "event_type": "human_decision",
             "summary": "Credit officer approves, overriding AI CONSIDER"},
        ],
        "audit_hash": "sha256:mock-c0bb1e-deadbeef-cafe1234",
    })
    print(f"~ LoanCase {CASE_ID}: updated human_decision_id, tmd_event_id, closed_at, decision_events")


if __name__ == "__main__":
    main()
