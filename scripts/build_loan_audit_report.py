#!/usr/bin/env python3
"""Create the 'Loan case audit' report on the local Runtara instance.

Single-page audit artefact keyed on case_id (LoanCase.case_id) plus
loan_application_id for the raw L1 source rows. Re-running deletes any
existing report with the same slug first.
"""

import json
import os
import sys
import urllib.error
import urllib.request

LOCAL = os.environ.get("LOCAL_URL", "http://localhost:7001")
SLUG = "loan-case-audit"


def http(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json"} if body is not None else {}
    req = urllib.request.Request(f"{LOCAL}{path}", data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"null")


# ---- helpers ---------------------------------------------------------------

def case_filter():
    # LoanCase row primary key (UUID) — workflows currently don't populate the
    # human-readable case_id column, so we key off the auto-managed id.
    return {"op": "EQ", "arguments": ["id", {"filter": "case_id", "path": "value"}]}


def loan_case_id_filter():
    return {"op": "EQ", "arguments": ["loan_case_id", {"filter": "case_id", "path": "value"}]}


def dossier_id_filter():
    return {"op": "EQ", "arguments": ["dossier_id", {"filter": "dossier_id", "path": "value"}]}


def id_eq_human_decision_filter():
    return {"op": "EQ", "arguments": ["id", {"filter": "human_decision_id", "path": "value"}]}


def loan_app_filter():
    return {
        "op": "EQ",
        "arguments": ["loan_application_id", {"filter": "loan_application_id", "path": "value"}],
    }


def application_id_filter():
    return {
        "op": "EQ",
        "arguments": ["application_id", {"filter": "loan_application_id", "path": "value"}],
    }


def show_when_case():
    return {"exists": True, "filter": "case_id"}


def show_when_loan_app():
    return {"exists": True, "filter": "loan_application_id"}


def col(field, label=None, fmt=None, **extra):
    c = {"field": field}
    if label:
        c["label"] = label
    if fmt:
        c["format"] = fmt
    c.update(extra)
    return c


PILL_DECISION = {
    "APPROVED": "success",
    "DECLINED": "destructive",
    "CONSIDER": "warning",
    "REQUEST_MORE_INFO": "warning",
    "PENDING": "muted",
}
PILL_STATUS = {
    "intake": "muted",
    "data_collection": "default",
    "scoring": "default",
    "ai_review": "default",
    "human_review": "default",
    "decided": "success",
    "withdrawn": "muted",
}
PILL_SEVERITY = {
    "hard_fail": "destructive",
    "score_penalty": "warning",
    "flag": "warning",
    "info": "muted",
}
PILL_OUTCOME = {"pass": "success", "fail": "destructive", "n_a": "muted"}
PILL_DECISION_BAND = {
    "APPROVED": "success",
    "CONSIDER": "warning",
    "DECLINED": "destructive",
}


# ---- metric blocks (case header KPIs) --------------------------------------

def metric_block(block_id, schema, condition, value_field, label, fmt=None):
    alias = f"{value_field}_value"
    block = {
        "id": block_id,
        "type": "metric",
        "lazy": False,
        "source": {
            "schema": schema,
            "mode": "aggregate",
            "condition": condition,
            "filterMappings": [],
            "groupBy": [],
            "aggregates": [
                {"alias": alias, "op": "max", "field": value_field}
            ],
            "orderBy": [],
            "limit": 1,
        },
        "metric": {"valueField": alias, "label": label},
        "filters": [],
        "interactions": [],
    }
    if fmt:
        block["metric"]["format"] = fmt
    return block


def card_block(block_id, schema, condition, groups, *, title=None):
    """Single-row card. `groups` is a list of dicts:
        {"id": str, "title": str, "columns": int, "fields": [card_field(...)]}
    """
    block = {
        "id": block_id,
        "type": "card",
        "lazy": False,
        "source": {
            "schema": schema,
            "mode": "filter",
            "condition": condition,
            "filterMappings": [],
            "groupBy": [],
            "aggregates": [],
            "orderBy": [],
        },
        "card": {"groups": groups},
        "filters": [],
        "interactions": [],
    }
    if title:
        block["title"] = title
    return block


def cf(field, label=None, *, kind="value", fmt=None, pill_variants=None,
       collapsed=None, col_span=None, subcard=None, subtable=None):
    f = {"field": field}
    if label:
        f["label"] = label
    if kind != "value":
        f["kind"] = kind
    if fmt:
        f["format"] = fmt
    if pill_variants:
        f["pillVariants"] = pill_variants
    if collapsed is not None:
        f["collapsed"] = collapsed
    if col_span:
        f["colSpan"] = col_span
    if subcard:
        f["subcard"] = subcard
    if subtable:
        f["subtable"] = subtable
    return f


def stcol(field, label=None, *, fmt=None, pill_variants=None, align=None):
    c = {"field": field}
    if label:
        c["label"] = label
    if fmt:
        c["format"] = fmt
    if pill_variants:
        c["pillVariants"] = pill_variants
    if align:
        c["align"] = align
    return c


def cgrp(group_id, title, fields, columns=2, description=None):
    g = {"id": group_id, "title": title, "columns": columns, "fields": fields}
    if description:
        g["description"] = description
    return g


def table_block(block_id, schema, condition, columns, *, title=None,
                show_when=None, default_sort=None, paginated=False, limit=None):
    src = {
        "schema": schema,
        "mode": "filter",
        "condition": condition,
        "filterMappings": [],
        "groupBy": [],
        "aggregates": [],
        "orderBy": default_sort or [],
    }
    if limit is not None:
        src["limit"] = limit
    block = {
        "id": block_id,
        "type": "table",
        "lazy": False,
        "source": src,
        "table": {"columns": columns, "defaultSort": default_sort or []},
        "filters": [],
        "interactions": [],
    }
    if title:
        block["title"] = title
    if paginated:
        block["table"]["pagination"] = {
            "defaultPageSize": 50,
            "allowedPageSizes": [25, 50, 100, 200],
        }
    return block


# ---- block list ------------------------------------------------------------

blocks = []

# 0. List view block — pick a case to drill into
blocks.append({
    "id": "cases_list",
    "type": "table",
    "lazy": False,
    "source": {
        "schema": "LoanCase",
        "mode": "filter",
        "filterMappings": [],
        "groupBy": [],
        "aggregates": [],
        "orderBy": [{"field": "opened_at", "direction": "desc"}],
    },
    "table": {
        "columns": [
            col("id", "Case", descriptive=True),
            col("loan_application_id", "Application"),
            col("opened_at", "Opened", fmt="datetime"),
            col("closed_at", "Closed", fmt="datetime"),
            col("current_status", "Status", fmt="pill", pillVariants=PILL_STATUS),
            col("final_decision", "Final decision", fmt="pill", pillVariants=PILL_DECISION),
            col("rule_pack_version_at_decision", "Rule pack"),
            col("tmd_version_at_decision", "TMD"),
            col("current_owner", "Owner"),
        ],
        "defaultSort": [{"field": "opened_at", "direction": "desc"}],
        "pagination": {"defaultPageSize": 50, "allowedPageSizes": [25, 50, 100]},
    },
    "filters": [],
    "interactions": [
        {
            "id": "open_case",
            "trigger": {"event": "row_click"},
            "actions": [
                {"type": "set_filter", "filterId": "case_id", "valueFrom": "datum.id"},
                {"type": "set_filter", "filterId": "loan_application_id", "valueFrom": "datum.loan_application_id"},
                {"type": "set_filter", "filterId": "dossier_id", "valueFrom": "datum.dossier_id"},
                {"type": "set_filter", "filterId": "human_decision_id", "valueFrom": "datum.human_decision_id"},
                {"type": "navigate_view", "viewId": "detail"},
            ],
        }
    ],
})

# 1. Case header KPIs
blocks += [
    metric_block("kpi_status", "LoanCase", case_filter(), "current_status", "Status"),
    metric_block("kpi_final_decision", "LoanCase", case_filter(), "final_decision", "Final decision"),
    metric_block("kpi_scoring_band", "ScoringResult", dossier_id_filter(), "decision_band", "Scoring band"),
    metric_block("kpi_ai_decision", "AIRecommendation", dossier_id_filter(), "decision", "AI decision"),
    metric_block("kpi_human_decision", "HumanDecision", id_eq_human_decision_filter(), "decision", "Human decision"),
]

# 1b. Case header card
blocks.append(card_block(
    "case_header",
    "LoanCase",
    case_filter(),
    [
        cgrp("identity", "Identity", columns=3, fields=[
            cf("case_id", "Case", col_span=2),
            cf("id", "Row id"),
            cf("loan_application_id", "Application"),
            cf("current_owner", "Owner"),
        ]),
        cgrp("lifecycle", "Lifecycle", columns=3, fields=[
            cf("opened_at", "Opened", fmt="datetime"),
            cf("closed_at", "Closed", fmt="datetime"),
            cf("current_status", "Status", fmt="pill", pill_variants=PILL_STATUS),
            cf("final_decision", "Final decision", fmt="pill", pill_variants=PILL_DECISION),
        ]),
        cgrp("versions", "Version pins at decision", columns=3, fields=[
            cf("rule_pack_version_at_decision", "Rule pack"),
            cf("tmd_version_at_decision", "TMD"),
            cf("product_config_version_at_decision", "Product cfg"),
            cf("audit_hash", "Audit hash", col_span=3),
        ]),
    ],
))

# 2. Decision timeline card (LoanCase pointer ids + decision_events JSON)
blocks.append(card_block(
    "decision_timeline",
    "LoanCase",
    case_filter(),
    [
        cgrp("pointers", "Layer artefact pointers", columns=3, fields=[
            cf("dossier_id", "Dossier"),
            cf("scoring_result_id", "Scoring result"),
            cf("ai_recommendation_id", "AI recommendation"),
            cf("human_decision_id", "Human decision"),
            cf("tmd_event_id", "TMD event"),
        ]),
        cgrp("events", "Decision events", columns=1, fields=[
            cf("decision_events", "Events", kind="subtable", col_span=1, subtable={
                "columns": [
                    stcol("seq", "#", align="right"),
                    stcol("timestamp", "When", fmt="datetime"),
                    stcol("layer", "Layer", fmt="pill", pill_variants={
                        "L1": "default", "L2": "default",
                        "L3": "warning", "L4": "success",
                    }),
                    stcol("event_type", "Event"),
                    stcol("actor", "Actor"),
                    stcol("summary", "Summary"),
                ],
                "emptyLabel": "No decision events recorded yet.",
            }),
        ], description="Ordered timeline written by L1–L4 workflows."),
    ],
))

# 3. Layer 1 — Data collection
blocks.append(card_block(
    "l1_application",
    "LoanApplication",
    application_id_filter(),
    [
        cgrp("meta", "Submission", columns=4, fields=[
            cf("application_id", "Application"),
            cf("submitted_at", "Submitted", fmt="datetime"),
            cf("channel", "Channel"),
            cf("status", "Status"),
        ]),
        cgrp("structured", "Structured payload", columns=2, fields=[
            cf("applicant", "Applicant", kind="json"),
            cf("employment", "Employment", kind="json"),
            cf("financial_position", "Financial position", kind="json"),
            cf("requested_loan", "Requested loan", kind="json"),
            cf("collateral", "Collateral", kind="json"),
            cf("consent", "Consent", kind="json"),
            cf("self_attested_flags", "Self-attested flags", kind="json"),
            cf("uploaded_documents", "Uploaded documents", kind="json"),
        ]),
    ],
    title="Loan application",
))

blocks.append(card_block(
    "l1_experian",
    "ExperianReport",
    loan_app_filter(),
    [
        cgrp("meta", "Inquiry", columns=4, fields=[
            cf("inquiry_id", "Inquiry"),
            cf("inquiry_timestamp", "Pulled at", fmt="datetime"),
            cf("experian_product", "Product"),
            cf("status", "Status"),
        ]),
        cgrp("score", "Credit signals", columns=4, fields=[
            cf("credit_score", "Score"),
            cf("score_band", "Band"),
            cf("identity_verified", "ID verified"),
            cf("error_message", "Error"),
            cf("defaults_count", "Defaults"),
            cf("defaults_total_amount_aud", "Defaults total", fmt="currency"),
            cf("enquiries_last_6mo", "Enq 6mo"),
            cf("open_accounts", "Open acc"),
            cf("judgements_count", "Judgements"),
            cf("bankruptcies_count", "Bankruptcies"),
        ]),
        cgrp("raw", "Raw response", columns=1, fields=[
            cf("fraud_alerts", "Fraud alerts", kind="json"),
            cf("raw_response", "Verbatim response", kind="json", collapsed=True),
        ]),
    ],
    title="Experian report (mocked v1)",
))

PILL_BOOL_GOOD = {"true": "success", "false": "muted"}
PILL_BOOL_BAD = {"true": "destructive", "false": "success"}
PILL_RESIDENCY = {"citizen": "success", "permanent_resident": "success", "temporary": "warning"}
PILL_CREDIT_RATING = {
    "excellent": "success", "good": "success",
    "average": "warning", "fair": "warning",
    "poor": "destructive", "bad": "destructive",
}
PILL_INCOME_RELIABILITY = {
    "stable": "success", "regular": "success",
    "irregular": "warning", "volatile": "destructive",
}
PILL_LITERACY = {"high": "success", "medium": "default", "low": "warning"}
PILL_RISK_TOLERANCE = {"low": "success", "medium": "default", "high": "warning"}
PILL_BENCHMARK_COVERAGE = {"full": "success", "partial": "warning", "none": "destructive"}
PILL_TRUTH_BELIEF = {"believed_true": "success", "uncertain": "warning", "doubted": "destructive"}

PILL_RISK_BAND = {
    "low": "success", "medium": "warning", "high": "destructive", "severe": "destructive"
}

blocks.append(card_block(
    "l1_merkle",
    "MerkleScienceReport",
    loan_app_filter(),
    [
        cgrp("meta", "Inquiry", columns=4, fields=[
            cf("inquiry_id", "Inquiry"),
            cf("inquiry_timestamp", "Pulled at", fmt="datetime"),
            cf("status", "Status"),
            cf("error_message", "Error"),
        ]),
        cgrp("wallet", "Wallet", columns=4, fields=[
            cf("wallet_address", "Wallet", col_span=2),
            cf("wallet_chain", "Chain"),
            cf("collateral_asset", "Asset"),
            cf("first_seen", "First seen", fmt="datetime"),
            cf("last_seen", "Last seen", fmt="datetime"),
        ]),
        cgrp("risk", "Risk", columns=3, fields=[
            cf("risk_score", "Risk score", fmt="decimal"),
            cf("risk_band", "Risk band", fmt="pill", pill_variants=PILL_RISK_BAND),
            cf("sanctions_hit", "Sanctions hit"),
        ]),
        cgrp("raw", "Raw response", columns=1, fields=[
            cf("exposure_categories", "Exposure", kind="json"),
            cf("raw_response", "Verbatim response", kind="json", collapsed=True),
        ]),
    ],
    title="Merkle Science report (mocked v1)",
))

blocks.append(card_block(
    "l1_serviceability",
    "ServiceabilityCalc",
    loan_app_filter(),
    [
        cgrp("meta", "Calculation", columns=4, fields=[
            cf("calc_id", "Calc"),
            cf("calculated_at", "Calculated", fmt="datetime"),
            cf("calc_method_version", "Method ver"),
            cf("passes_serviceability", "Passes"),
        ]),
        cgrp("household", "Household", columns=3, fields=[
            cf("household_size", "Household"),
            cf("num_dependents", "Dependents"),
            cf("location_tier", "Location"),
            cf("hem_benchmark_id", "HEM benchmark id"),
            cf("hem_monthly_amount_aud", "HEM monthly", fmt="currency"),
        ]),
        cgrp("income", "Income & expenses", columns=3, fields=[
            cf("annual_gross_income_aud", "Annual gross", fmt="currency"),
            cf("monthly_net_income_aud", "Monthly net", fmt="currency"),
            cf("declared_monthly_expenses_aud", "Declared expenses", fmt="currency"),
            cf("applied_expenses_aud", "Applied expenses", fmt="currency"),
            cf("existing_debt_monthly_aud", "Existing debt", fmt="currency"),
            cf("loan_repayment_modelled_aud", "Modelled repayment", fmt="currency"),
        ]),
        cgrp("surplus", "Surplus & stress", columns=3, fields=[
            cf("monthly_surplus_aud", "Surplus", fmt="currency"),
            cf("stress_buffer_pct", "Stress buffer", fmt="percent"),
            cf("monthly_surplus_stressed_aud", "Surplus stressed", fmt="currency"),
        ]),
        cgrp("raw", "Inputs snapshot", columns=1, fields=[
            cf("calc_inputs_snapshot", "Calc inputs", kind="json", collapsed=True),
        ]),
    ],
    title="Serviceability calc",
))

blocks.append(card_block(
    "l1_dossier",
    "ApplicantDossier",
    loan_app_filter(),
    [
        cgrp("ids", "Identity & sources", columns=3, fields=[
            cf("dossier_id", "Dossier"),
            cf("loan_case_id", "Loan case id"),
            cf("loan_application_id", "Application"),
            cf("compiled_at", "Compiled", fmt="datetime"),
            cf("experian_report_id", "Experian id"),
            cf("serviceability_calc_id", "Serviceability id"),
            cf("merkle_science_report_ids", "Merkle ids", kind="json"),
            cf("source_versions", "Source versions", kind="json"),
            cf("data_quality_flags", "Data quality", kind="json"),
        ]),
        cgrp("applicant", "Applicant", columns=1, fields=[
            cf("applicant_summary", "Applicant snapshot", kind="subcard", subcard={
                "groups": [{
                    "id": "applicant_inner", "columns": 4, "fields": [
                        cf("age", "Age"),
                        cf("residency_status", "Residency", fmt="pill", pill_variants=PILL_RESIDENCY),
                        cf("substantial_hardship", "Substantial hardship", fmt="pill", pill_variants=PILL_BOOL_BAD),
                        cf("unable_to_comply", "Unable to comply", fmt="pill", pill_variants=PILL_BOOL_BAD),
                    ],
                }],
            }),
        ]),
        cgrp("financial", "Financial position", columns=1, fields=[
            cf("financial_summary", "Financial summary", kind="subcard", subcard={
                "groups": [
                    {"id": "income", "title": "Income & surplus", "columns": 4, "fields": [
                        cf("annual_gross_income_aud", "Annual gross", fmt="currency"),
                        cf("monthly_surplus_aud", "Monthly surplus", fmt="currency"),
                        cf("liquid_assets_aud", "Liquid assets", fmt="currency"),
                        cf("existing_debt_monthly_aud", "Existing debt / mo", fmt="currency"),
                        cf("discretionary_outgoings_aud", "Discretionary / mo", fmt="currency"),
                        cf("income_reliability_rating", "Reliability", fmt="pill", pill_variants=PILL_INCOME_RELIABILITY),
                        cf("social_security_income_percentage", "Social-sec income %", fmt="percent"),
                    ]},
                    {"id": "benchmark", "title": "HEM benchmark", "columns": 4, "fields": [
                        cf("benchmark_figure", "Figure", fmt="currency"),
                        cf("benchmark_tier", "Tier"),
                        cf("benchmark_conservatism", "Conservatism"),
                        cf("benchmark_expense_coverage", "Coverage", fmt="pill", pill_variants=PILL_BENCHMARK_COVERAGE),
                        cf("benchmark_version_date", "Version", fmt="date"),
                        cf("benchmark_income_adjustment", "Income adjustment", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                        cf("expense_buffer_applied", "Buffer applied", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                    ]},
                    {"id": "behaviour", "title": "Behaviour flags", "columns": 3, "fields": [
                        cf("transaction_history_90_days", "90-day txn hist", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                        cf("verified_hem_excluded_expenses", "Verified HEM exclusions", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                        cf("verified_non_benchmark_expenses", "Verified non-benchmark", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                        cf("has_overdrawn_savings_or_reversed_dd", "Overdraw / reversed DD", fmt="pill", pill_variants=PILL_BOOL_BAD),
                        cf("income_withdrawn_early", "Income withdrawn early", fmt="pill", pill_variants=PILL_BOOL_BAD),
                    ]},
                ],
            }),
        ]),
        cgrp("credit", "Credit", columns=1, fields=[
            cf("credit_summary", "Credit summary", kind="subcard", subcard={
                "groups": [{"id": "credit_inner", "columns": 4, "fields": [
                    cf("credit_score", "Score"),
                    cf("credit_management_rating", "Management", fmt="pill", pill_variants=PILL_CREDIT_RATING),
                    cf("active_credit_obligations_count", "Active obligations"),
                    cf("defaults_count", "Defaults"),
                    cf("judgements_count", "Judgements"),
                    cf("bankruptcies_count", "Bankruptcies"),
                    cf("net_debt_trend_increasing", "Debt trend ↑", fmt="pill", pill_variants=PILL_BOOL_BAD),
                    cf("savings_history_evidence", "Savings history", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                    cf("utilities_delinquencies_count", "Utility delinquencies"),
                    cf("current_small_amount_credit_contract_default", "Active SACC default", fmt="pill", pill_variants=PILL_BOOL_BAD),
                    cf("sacc_debtor_count_last_90_days", "SACC debtors 90d"),
                    cf("sacc_defaults_in_last_90_days", "SACC defaults 90d"),
                    cf("small_amount_credit_contracts_last_90_days_count", "SACC contracts 90d"),
                ]}],
            }),
        ]),
        cgrp("collateral", "Collateral", columns=1, fields=[
            cf("collateral_summary", "Collateral summary", kind="subcard", subcard={
                "groups": [{"id": "collateral_inner", "columns": 3, "fields": [
                    cf("asset", "Asset"),
                    cf("pledged_amount", "Pledged amount", fmt="decimal"),
                    cf("pledged_aud_value", "Pledged AUD value", fmt="currency"),
                    cf("requested_lvr", "Requested LVR", fmt="percent"),
                    cf("aml_risk_band", "AML risk band", fmt="pill", pill_variants=PILL_RISK_BAND),
                    cf("sanctions_hit", "Sanctions hit", fmt="pill", pill_variants=PILL_BOOL_BAD),
                ]}],
            }),
        ]),
        cgrp("tmd", "TMD eligibility", columns=1, fields=[
            cf("tmd_eligibility_inputs", "Eligibility inputs", kind="subcard", subcard={
                "groups": [{"id": "tmd_inner", "columns": 5, "fields": [
                    cf("is_18_or_older", "18 or older", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                    cf("residency_ok", "Residency", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                    cf("employment_ok", "Employment", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                    cf("has_au_bank_account", "AU bank account", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                    cf("collateral_ok", "Collateral", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                ]}],
            }),
        ]),
        cgrp("requested", "Requested loan", columns=1, fields=[
            cf("requested_loan", "Requested loan", kind="subcard", subcard={
                "groups": [{"id": "req_inner", "columns": 4, "fields": [
                    cf("product_id", "Product", col_span=2),
                    cf("loan_amount_aud", "Amount", fmt="currency"),
                    cf("term_months", "Term (mo)"),
                    cf("purpose", "Purpose"),
                    cf("repayment_to_income_ratio", "Repayment / income", fmt="percent"),
                    cf("is_complex", "Complex", fmt="pill", pill_variants=PILL_BOOL_BAD),
                    cf("is_split_into_multiple_contracts", "Split contracts", fmt="pill", pill_variants=PILL_BOOL_BAD),
                    cf("are_multiple_contracts_more_expensive", "Splits cost more", fmt="pill", pill_variants=PILL_BOOL_BAD),
                ]}],
            }),
        ]),
        cgrp("attested", "Self-attested flags", columns=1, fields=[
            cf("self_attested_flags", "Self-attested flags", kind="subcard", subcard={
                "groups": [
                    {"id": "fit", "title": "Fit & objectives", "columns": 4, "fields": [
                        cf("crypto_literacy", "Crypto literacy", fmt="pill", pill_variants=PILL_LITERACY),
                        cf("risk_tolerance", "Risk tolerance", fmt="pill", pill_variants=PILL_RISK_TOLERANCE),
                        cf("meets_objectives", "Meets objectives", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                        cf("meets_requirements", "Meets requirements", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                        cf("features_benefits_considered", "Considered features", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                        cf("understands_obligations", "Understands obligations", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                        cf("has_difficulty_understanding", "Difficulty understanding", fmt="pill", pill_variants=PILL_BOOL_BAD),
                        cf("trades_crypto_regularly", "Trades crypto regularly", fmt="pill", pill_variants=PILL_BOOL_BAD),
                    ]},
                    {"id": "repay", "title": "Repayment plan", "columns": 4, "fields": [
                        cf("has_repayment_plan", "Has plan", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                        cf("has_repayment_plan_without_selling_principal_residence", "Plan w/o selling residence", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                        cf("uses_secured_property_for_repayment", "Uses secured property", fmt="pill", pill_variants=PILL_BOOL_BAD),
                        cf("can_repay_credit_limit_within_asic_period", "Within ASIC period", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                        cf("likely_to_sell_assets", "Likely to sell assets", fmt="pill", pill_variants=PILL_BOOL_BAD),
                        cf("requires_spending_reductions", "Requires spending cuts", fmt="pill", pill_variants=PILL_BOOL_BAD),
                        cf("willing_to_reduce_spending", "Willing to reduce spending", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                        cf("accepts_additional_costs_risks", "Accepts additional costs", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                    ]},
                    {"id": "consistency", "title": "Consistency & risk", "columns": 4, "fields": [
                        cf("inconsistencies_detected", "Inconsistencies detected", fmt="pill", pill_variants=PILL_BOOL_BAD),
                        cf("inconsistent_features_removed", "Inconsistent features removed", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                        cf("no_inconsistent_features", "No inconsistent features", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                        cf("substantial_hardship_risk", "Substantial hardship risk", fmt="pill", pill_variants=PILL_BOOL_BAD),
                        cf("red_flags_financial_vulnerability", "Vulnerability red flags", fmt="pill", pill_variants=PILL_BOOL_BAD),
                        cf("obtains_limited_benefit", "Limited benefit", fmt="pill", pill_variants=PILL_BOOL_BAD),
                    ]},
                ],
            }),
        ]),
        cgrp("assessment", "Assessment", columns=2, fields=[
            cf("assessment_summary", "Assessment summary", kind="subcard", subcard={
                "groups": [{"id": "assess_inner", "columns": 2, "fields": [
                    cf("assessment_period", "Assessment period"),
                    cf("information_truth_belief", "Truth belief", fmt="pill", pill_variants=PILL_TRUTH_BELIEF),
                    cf("doubt_about_information", "Doubt", fmt="pill", pill_variants=PILL_BOOL_BAD),
                    cf("irrelevant_information_used", "Irrelevant info used", fmt="pill", pill_variants=PILL_BOOL_BAD),
                ]}],
            }),
            cf("assessment_data", "Assessment data", kind="subcard", subcard={
                "groups": [{"id": "data_inner", "columns": 2, "fields": [
                    cf("category", "Category"),
                    cf("verified", "Verified", fmt="pill", pill_variants=PILL_BOOL_GOOD),
                ]}],
            }),
            cf("assessment", "Assessment (raw)", kind="json", collapsed=True),
            cf("third_party_intermediary", "Third-party intermediary", kind="json", collapsed=True),
            cf("external_fraud_investigations", "External fraud investigations", kind="json", collapsed=True),
            cf("rule_results", "Rule results", kind="json", collapsed=True),
        ]),
    ],
    title="Applicant dossier",
))

# 4. Layer 2 — scoring header
blocks.append(card_block(
    "l2_scoring_header",
    "ScoringResult",
    dossier_id_filter(),
    [
        cgrp("result", "Scoring result", columns=4, fields=[
            cf("scoring_result_id", "Scoring result", col_span=2),
            cf("scored_at", "Scored at", fmt="datetime"),
            cf("decision_band", "Band", fmt="pill", pill_variants=PILL_DECISION_BAND),
            cf("weighted_score", "Weighted score", fmt="decimal"),
            cf("passes_serviceability", "Passes serviceability"),
            cf("tmd_within_target_market", "TMD within target market"),
            cf("scoring_method_version", "Method ver"),
            cf("rule_pack_version", "Rule pack"),
        ]),
        cgrp("reason", "Reason summary", columns=1, fields=[
            cf("decision_reason_summary", "Reason"),
        ]),
        cgrp("flags", "Triggered rule sets", columns=2, fields=[
            cf("hard_fails", "Hard fails", kind="json"),
            cf("flags", "Flags", kind="json"),
            cf("active_rule_ids", "Active rule ids", kind="json", collapsed=True, col_span=2),
        ]),
    ],
    title="Layer 2 — scoring header",
))

# 4b. Rule outcome trace (122 rows for active rules)
blocks.append(table_block(
    "l2_rule_outcomes",
    "RuleOutcome",
    dossier_id_filter(),
    [
        col("severity", "Severity", fmt="pill", pillVariants=PILL_SEVERITY),
        col("category", "Category"),
        col("rule_id", "Rule", descriptive=True),
        col("rule_manifest_version", "Rule v"),
        col("rule_workflow_id", "Workflow id"),
        col("rule_workflow_version", "WF v"),
        col("execution_instance_id", "Instance"),
        col("applied", "Applied"),
        col("outcome", "Outcome", fmt="pill", pillVariants=PILL_OUTCOME),
        col("contribution", "Contribution", fmt="decimal", align="right"),
        col("message", "Message"),
        col("source_quote", "Source quote"),
        col("evaluated_at", "Evaluated", fmt="datetime"),
        col("input_snapshot", "Inputs (JSON)"),
    ],
    title="Layer 2 — rule outcome trace",
    default_sort=[
        {"field": "severity", "direction": "asc"},
        {"field": "category", "direction": "asc"},
        {"field": "rule_id", "direction": "asc"},
    ],
    paginated=True,
))

# 5. Layer 3 — AI review
blocks.append(card_block(
    "l3_ai_recommendation",
    "AIRecommendation",
    dossier_id_filter(),
    [
        cgrp("verdict", "Verdict", columns=4, fields=[
            cf("ai_recommendation_id", "Recommendation", col_span=2),
            cf("produced_at", "Produced", fmt="datetime"),
            cf("decision", "Decision", fmt="pill", pill_variants=PILL_DECISION),
            cf("confidence_score", "Confidence", fmt="decimal"),
            cf("citation_check_passed", "Citations OK"),
        ]),
        cgrp("model", "Model & cost", columns=4, fields=[
            cf("agent_id", "Agent"),
            cf("model_id", "Model"),
            cf("prompt_version", "Prompt v"),
            cf("tokens_in", "Tokens in"),
            cf("tokens_out", "Tokens out"),
        ]),
        cgrp("reasoning", "Reasoning & risks", columns=2, fields=[
            cf("reasons", "Reasons", kind="json"),
            cf("risk_flags", "Risk flags", kind="json"),
            cf("recommended_actions", "Recommended actions", kind="json"),
        ]),
        cgrp("comms", "Draft communications", columns=1, fields=[
            cf("draft_email", "Draft email", kind="markdown", collapsed=True),
        ]),
        cgrp("raw", "Raw agent output", columns=1, fields=[
            cf("raw_agent_output", "Raw output", kind="json", collapsed=True),
        ]),
    ],
))

# 6. Layer 4 — Human decision
blocks.append(card_block(
    "l4_human_decision",
    "HumanDecision",
    id_eq_human_decision_filter(),
    [
        cgrp("verdict", "Verdict", columns=4, fields=[
            cf("human_decision_id", "Decision", col_span=2),
            cf("decided_at", "Decided", fmt="datetime"),
            cf("decision", "Decision", fmt="pill", pill_variants=PILL_DECISION),
            cf("reviewer_id", "Reviewer"),
            cf("reviewer_role", "Role"),
            cf("overrode_ai", "Overrode AI"),
            cf("overrode_scoring", "Overrode scoring"),
        ]),
        cgrp("reasoning", "Reasoning", columns=1, fields=[
            cf("override_reasoning", "Override reasoning", kind="markdown"),
            cf("reviewer_notes", "Reviewer notes", kind="markdown"),
            cf("info_requested", "Info requested", kind="json"),
        ]),
        cgrp("links", "Audit links", columns=2, fields=[
            cf("signal_response_id", "Signal response id"),
            cf("report_snapshot_ref", "Report snapshot ref"),
        ]),
    ],
))

# 7. TMD distribution event (referenced via final_decision_outcome)
blocks.append(card_block(
    "tmd_event",
    "TMDDistributionEvent",
    loan_case_id_filter(),
    [
        cgrp("event", "Event", columns=4, fields=[
            cf("tmd_event_id", "TMD event", col_span=2),
            cf("assessed_at", "Assessed", fmt="datetime"),
            cf("tmd_version", "TMD version"),
            cf("within_target_market", "Within target market"),
            cf("is_significant_dealing", "Significant dealing"),
            cf("final_decision_outcome", "Outcome", fmt="pill", pill_variants=PILL_DECISION),
            cf("reported_to_compliance_at", "Reported", fmt="datetime"),
        ]),
        cgrp("rationale", "Rationale", columns=1, fields=[
            cf("significant_dealing_rationale", "Significant dealing rationale", kind="markdown"),
        ]),
        cgrp("details", "Details", columns=2, fields=[
            cf("eligibility_outcomes", "Eligibility outcomes", kind="json"),
            cf("negative_market_flags", "Negative market flags", kind="json"),
        ]),
    ],
))

# 8. Replay metadata — version pins on LoanCase
blocks.append(card_block(
    "replay_metadata",
    "LoanCase",
    case_filter(),
    [
        cgrp("versions", "Version pins", columns=3, fields=[
            cf("rule_pack_version_at_decision", "Rule pack version"),
            cf("tmd_version_at_decision", "TMD version"),
            cf("product_config_version_at_decision", "Product config version"),
        ]),
        cgrp("artefacts", "Layer artefacts", columns=3, fields=[
            cf("dossier_id", "Dossier id"),
            cf("scoring_result_id", "Scoring result id"),
            cf("ai_recommendation_id", "AI recommendation id"),
            cf("human_decision_id", "Human decision id"),
            cf("tmd_event_id", "TMD event id"),
            cf("audit_hash", "Audit hash"),
        ]),
    ],
))


# ---- layout ----------------------------------------------------------------

def section(node_id, title, children, description=None):
    n = {"id": node_id, "type": "section", "title": title, "children": children}
    if description:
        n["description"] = description
    return n


def block_node(block_id):
    return {"id": f"{block_id}_node", "type": "block", "blockId": block_id}


def metric_row(node_id, blocks):
    return {"id": node_id, "type": "metric_row", "blocks": blocks}


detail_layout = [
    section("sec_header", "1. Case header", [
        metric_row("kpis", [
            "kpi_status", "kpi_final_decision", "kpi_scoring_band",
            "kpi_ai_decision", "kpi_human_decision",
        ]),
        block_node("case_header"),
    ]),
    section("sec_timeline", "2. Decision timeline", [
        block_node("decision_timeline"),
    ], description="Ordered events written by L1–L4 workflows. Shown as JSON until the LoanCase decision_events flattening view ships."),
    section("sec_l1", "3. Layer 1 — Data collection", [
        block_node("l1_application"),
        block_node("l1_experian"),
        block_node("l1_merkle"),
        block_node("l1_serviceability"),
        block_node("l1_dossier"),
    ], description="Persisted source rows feeding the dossier. Raw responses are exposed as JSON cells for verbatim audit."),
    section("sec_l2", "4. Layer 2 — Deterministic scoring", [
        block_node("l2_scoring_header"),
        block_node("l2_rule_outcomes"),
    ]),
    section("sec_l3", "5. Layer 3 — AI review", [
        block_node("l3_ai_recommendation"),
    ]),
    section("sec_l4", "6. Layer 4 — Human decision", [
        block_node("l4_human_decision"),
    ]),
    section("sec_tmd", "7. TMD distribution event", [
        block_node("tmd_event"),
    ]),
    section("sec_replay", "8. Replay metadata", [
        block_node("replay_metadata"),
    ], description="Version pins needed to re-execute this case end-to-end."),
]

views = [
    {
        "id": "list",
        "title": "Loan cases",
        "layout": [block_node("cases_list")],
    },
    {
        "id": "detail",
        "titleFromBlock": {"block": "case_header", "field": "id"},
        "breadcrumb": [
            {"label": "Loan cases", "viewId": "list", "clearFilters": ["case_id", "loan_application_id"]}
        ],
        "layout": detail_layout,
    },
]


# ---- filters ---------------------------------------------------------------

case_blocks_by_id = ["case_header", "decision_timeline", "replay_metadata"]
case_blocks_by_loan_case_id = [
    "l2_scoring_header", "l2_rule_outcomes", "l3_ai_recommendation",
    "l4_human_decision", "tmd_event",
    "kpi_scoring_band", "kpi_ai_decision", "kpi_human_decision",
]
case_blocks_kpi_id = ["kpi_status", "kpi_final_decision"]
loan_app_blocks_application_id = ["l1_application"]
loan_app_blocks_loan_application_id = [
    "l1_experian", "l1_merkle", "l1_serviceability", "l1_dossier",
]

# Navigation-only text filters (no UI chips); set by the cases_list row-click
# interaction, then read by detail-view block conditions.
filters = [
    {"id": "case_id", "label": "Case", "type": "text", "required": False,
     "strictWhenReferenced": True, "appliesTo": []},
    {"id": "loan_application_id", "label": "Loan application", "type": "text",
     "required": False, "strictWhenReferenced": True, "appliesTo": []},
    {"id": "dossier_id", "label": "Dossier", "type": "text",
     "required": False, "strictWhenReferenced": True, "appliesTo": []},
    {"id": "human_decision_id", "label": "Human decision", "type": "text",
     "required": False, "strictWhenReferenced": True, "appliesTo": []},
]


# ---- definition + create ---------------------------------------------------

definition = {
    "definitionVersion": 1,
    "markdown": "",
    "filters": filters,
    "views": views,
    "blocks": blocks,
}

payload = {
    "name": "Loan case audit",
    "slug": SLUG,
    "description": "Single-page, regulator-shareable audit artefact for a closed loan case (RG 209 reconstruction).",
    "tags": ["lending", "audit", "compliance"],
    "status": "published",
    "definition": definition,
}


def find_existing():
    s, d = http("GET", "/api/runtime/reports")
    if s != 200:
        raise SystemExit(f"list reports failed: {s} {d}")
    for r in d.get("reports", []):
        if r["slug"] == SLUG:
            return r["id"]
    return None


def main():
    # Validate first
    s, d = http("POST", "/api/runtime/reports/validate", {"definition": definition})
    if s != 200:
        raise SystemExit(f"validate request failed: {s} {d}")
    if not d.get("valid"):
        print("validation errors:", json.dumps(d.get("errors"), indent=2), file=sys.stderr)
        sys.exit(1)
    if d.get("warnings"):
        print("validation warnings:", json.dumps(d["warnings"], indent=2))

    existing = find_existing()
    if existing:
        s, d = http("PUT", f"/api/runtime/reports/{existing}", payload)
        if s not in (200, 201):
            raise SystemExit(f"update failed: {s} {json.dumps(d, indent=2)}")
        print(f"updated report {existing} slug={SLUG}")
    else:
        s, d = http("POST", "/api/runtime/reports", payload)
        if s not in (200, 201):
            raise SystemExit(f"create failed: {s} {json.dumps(d, indent=2)}")
        print(f"created report {d['report']['id']} slug={d['report']['slug']}")


if __name__ == "__main__":
    main()
