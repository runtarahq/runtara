# Inventory pilot

An isolated, runnable Runtara PoC for a multi-warehouse ecommerce operation.
It starts with fresh, dedicated databases and synthetic data. It does not use
the existing development databases or any real business credentials.

## Open the running pilot

- Native Runtara reports: <http://localhost:3080/ui/reports>
- Workflow designer and history: <http://localhost:3080/ui/workflows>
- Native object data: <http://localhost:3080/ui/objects/types>

Use the three reports: **Control tower**, **Purchasing and freight**, and
**Integration health and audit**. They contain charts, metrics, tables, and
native workflow launch buttons. Runtara is the sole user interface. The mock
external-system adapter is internal to Docker and has no published port.

The server is pinned to the already available Runtara 8.9.6 image by digest.
All published ports bind to `127.0.0.1`. Authentication is local evaluation
mode. PostgreSQL, Valkey, runtime artifacts, and the mock ledger each have
dedicated `runtara-inventory-poc_*` volumes. No real integrations are configured.

## Try it

1. Open the control tower report. The delivered instance is already
   populated with a completed pilot and execution history.
2. Use **Reset synthetic data** in that report for a clean walkthrough. This restores
   synthetic opening balances and clears only the mock operational ledger;
   workflow definitions, reports, and Runtara execution history remain.
3. Run **Run full pilot**, or scenarios 01–08 in sequence. Wait for completion.
4. Refresh a native report after a workflow finishes to see every block update.
5. Run scenario 06 repeatedly: stock must not change. Scenario 07 fails once
   with HTTP 503, then applies its receipt once; subsequent runs are duplicates.
6. Run scenario 10: it deliberately fails with an over-receipt and leaves stock
   unchanged. The failure is visible in workflow history and the adapter audit.
7. Scenario 08 creates stale WMS data and a seven-unit discrepancy. Scenario 09
   simulates operator confirmation of the WMS snapshot, resolves the alerts,
   and does not silently adjust ledger inventory.

The predefined event identities are fixed. Repeating a completed scenario
replays the same events; it does not create a new business transaction. After
resolving scenario 08, reset the demo or use new custom event keys to inject a
new discrepancy. A full pilot replay is also safe and idempotent.

### Exact acceptance balances

The purchase order is for 3,000 BOT-750 units at $12/unit:

| Shipment | Destination | Shipped | Received | Damaged receipts | In transit | Freight | Landed/unit |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| AIR-500 | Amsterdam | 500 | 400 | 5 | 100 | $1,500 | $15 |
| SEA-2000 | Singapore | 2,000 | 600 | 12 | 1,400 | $2,000 | $13 |

**500 supplier + 1,500 transit + 1,000 confirmed receipts = 3,000 ordered.**
Damaged receipts are a subset of receipts, not another PO quantity bucket.
Transit never contributes to warehouse availability.

The transfer moves 40 units out of Amsterdam, of which Singapore receives 25;
15 remain in transfer transit. Returns add 8 sellable units to Amsterdam and
2 damaged units to Singapore. A customer order reserves 30 units at Amsterdam.

| All three SKUs, both warehouses | Final units |
| --- | ---: |
| Physical | 1,375 |
| Reserved | 70 |
| Damaged | 23 |
| Available = physical − reserved − damaged | 1,282 |

BOT-750 physical balances are Amsterdam **468**, Singapore **707**. Other
products have 200 physical units total. These differ from cumulative PO
receipts because opening inventory, returns and transfer transit are separate.

Synthetic opening balances: BOT-750 AMS 100 / SIN 80 (10 reserved and 2 damaged
at each); MUG-350 60 at each (5 reserved); TOTE-01 40 at each (5 reserved).
Opening balances are internally validated fixtures, not verified real counts.
Daily demand is illustrative; coverage and 30-day replenishment are estimates.

### Custom events

Open workflow **11 · Process a custom inventory event** in Runtara and click
Run. Enter a JSON string in its `event` field, matching the installed HTTP-agent
contract. For example, the field contents can be:

```json
{"key":"receipt-air-manual-001","type":"receipt","shipment":"AIR-500","quantity":25,"damaged":0}
```

After the complete pilot, another 100 air units remain receivable. Use a new
key for a new receipt, or repeat the identical key and payload to test
deduplication. Changing a payload under an existing key is rejected.

External identifiers can be normalized through the synthetic SKU crosswalk:

```json
{"key":"store-order-2001","type":"reserve","fc":"AMS","system":"store","externalSku":"SHOP-MUG","quantity":5}
```

Supported systems: `supplier`, `store`, `warehouse`. Unknown or conflicting
mappings are rejected for manual review. See `scenarios.mjs` for event examples
covering dispatch, receipt, reservation, transfer dispatch/receipt, returns,
mock drift, reconciliation, and resolution. The mock uses one PO and one SKU
per inbound shipment; a production adapter needs arbitrary POs and SKU lines.

## Reproduce, stop, resume

From the repository root, with Docker running:

```sh
# The pinned server image is already cached on the delivery machine.
# On another machine, pull that exact image first:
docker pull ghcr.io/runtarahq/runtara@sha256:edcaabfb323ffd82a8a4b0942eceb6a42f9e41b797199be5f45bae67f31148ac
docker compose -f examples/inventory-poc/compose.yml up -d
docker compose -f examples/inventory-poc/compose.yml exec -T inventory node seed.mjs
```

Seeding preserves existing mock quantities, reuses registered workflows and
schemas, and updates the three report definitions. It does not run the demo.
Wait for the Runtara health check before seeding. On the delivered machine the
containers are already running. Initial workflow execution may spend several
seconds queued before an instance is visible. Native report actions show the
running instance and its completion; execution history is available in Runtara.

```sh
# Stop only this stack, preserving its volumes:
docker compose -f examples/inventory-poc/compose.yml stop
# Resume only this stack:
docker compose -f examples/inventory-poc/compose.yml start
```

To edit adapter code, restart only its service. Report/scenario authoring is in
`seed.mjs` and `scenarios.mjs`; registered compiled workflows are deliberately
not overwritten by a regular seed rerun. Create an updated version through
Runtara when modifying an existing graph, or explicitly select scenario keys:

```sh
docker compose -f examples/inventory-poc/compose.yml exec -T -e UPDATE_WORKFLOWS=demo,retry inventory node seed.mjs
```

## Verification

```sh
docker compose -f examples/inventory-poc/compose.yml exec -T inventory node --test ledger.test.mjs
# WARNING: acceptance test resets this PoC's synthetic ledger first.
docker compose -f examples/inventory-poc/compose.yml exec -T inventory node verify.mjs
```

The E2E script uses real asynchronous Runtara workflow executions and checks
exact quantities, partial transfer conservation, duplicate replay, one-time
503 recovery, invalid quantity rejection, idempotency conflicts, alert
resolution, and rendering of every native report block. It leaves representative
alerts visible so the integration-health report has useful demonstration data.

No Rust or production frontend source was changed. Workspace Cargo checks,
production frontend build/lint, real connector tests, and production load tests
are outside this example's verification boundary.

## Architecture and ownership

- **Native Runtara:** workflow compilation and durable execution, HTTP agent,
  error edges, step events, history, Object Model schemas/rows, report charts,
  report workflow actions, and table/metric rendering.
- **Custom PoC code:** inventory invariants, canonical SKU resolution,
  idempotency, mock WMS failure injection, reconciliation, freight allocation,
  synthetic fixtures and report projection. There is no custom user interface.
- **Mocked:** supplier, ecommerce, warehouses, carrier references/ETAs, and
  inventory-platform APIs. No Cin7-specific API compatibility is claimed.
- **Not evaluated:** paid connector availability/pricing, vendor-specific
  partial-shipment APIs, accounting valuation, duties, currency conversion,
  manufacturing/kits, lot/serial tracking, authentication/roles, HA, event
  throughput, and six-center/twenty-market rollout readiness.

The mock adapter has a single serialized writer. Successful business events
are applied to a clone, validated, and saved by file fsync + atomic rename.
Rejected events do not change quantities. The event payload fingerprint and
stock change share that state commit. State survives ordinary container restarts.
This is a demonstration persistence model, not a production inventory database.
Use a transactional database, unique source event constraints, and an outbox
for production. Report projection updates native rows separately and can show
a briefly mixed snapshot; it is not a distributed atomic transaction. It can
be retried using `/publish`, without reapplying inventory events. A crash between
a native row creation and saving its returned ID requires projection repair.

## First-week pilot and rollout

The realistic one-week deliverable is a **bounded pilot**, not a proven cutover
of all 60,000 monthly orders, 20 markets, six fulfillment centers and 100 SKUs.

| Day | Deliverable / decision |
| --- | --- |
| 1 | Map sources of truth, order/stock states, IDs, owners and exception paths; obtain API sandboxes and sample exports. |
| 2 | Confirm SKU crosswalk; reconcile signed-off opening balances and outstanding POs for two centers and representative SKUs. |
| 3 | Prove split shipments/partial receipts against the inventory vendor's actual sandbox; document native vs paid connector vs custom work. |
| 4 | Connect one storefront and two WMS sandboxes; test duplicates, timeouts, invalid SKUs, out-of-order events and reconciliation. |
| 5 | Operator UAT using the acceptance matrix; train exception/retry procedures; agree go/no-go and phased rollout backlog. |

Required inputs: current system diagram and exports; canonical SKU and supplier
IDs; locations/stock statuses and units of measure; actual on-hand, reserved,
damaged and in-transit balances; all open PO lines and shipment/receipt IDs;
WMS/store API documentation and sandbox access through secure connection setup;
peak event rates and rate limits; costing/accounting policies; one operations
owner and one technical contact available daily. Do not put credentials in the
seed files or workflow inputs.

Cin7 remains the user's evaluation candidate. This PoC supplies vendor acceptance
tests; it is not sufficient evidence to recommend buying Cin7 or an alternative.
Require a vendor sandbox demonstration of the exact 3,000-unit scenario and
document feature/plan/API restrictions before platform selection. An inventory
system should own stock and costing; Runtara can orchestrate integration and
exceptions. This local adapter temporarily stands in for that system.

Roll out by warehouse/store cohort after reconciliation and parallel-run signoff.
Assign daily discrepancy ownership, monitor data age and failed jobs, keep
operator-approved corrections auditable, and agree retry limits, escalation
windows, backup/restore drills and on-call support. Training should cover the
three reports, the failed-run history, identical-payload retries, mapping review,
and controlled reconciliation.

Commercial rates, availability, prior implementation references and fixed-price
estimates must come from the implementing consultant. None are fabricated here.
Budget discovery, the two-center pilot and rollout separately; estimate only
after connector/API gaps, data quality and peak throughput are measured.
