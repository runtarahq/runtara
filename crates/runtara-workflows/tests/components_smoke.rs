//! Components-mode per-shape smoke tests.
//!
//! Mirrors the rustc-legacy `smoke_compile.rs` coverage for the components
//! pipeline: each workflow shape (Finish, Split, Conditional, Switch, While,
//! Log, Delay, Error, WaitForSignal, EmbedWorkflow, multi-agent, …) is
//! compiled end-to-end through `cargo component build` + `wac compose`, and
//! the resulting composed `workflow.wasm` is asserted to exist + be a
//! non-empty Component-Model artifact.
//!
//! Gated by `RUNTARA_RUN_COMPONENTS_E2E=1`. The first shape pays the cold
//! cargo-component build (~30s); subsequent shapes reuse the same
//! `CARGO_TARGET_DIR` (via `RUNTARA_COMPONENTS_TARGET_DIR`) so they ride on
//! the incremental cache and finish in 2-5s each. Total wall-clock for the
//! full suite locally is ~60-90s on a warm machine.

use std::path::{Path, PathBuf};
use std::process::Command;

use runtara_workflows::{ChildWorkflowInput, CompilationInput, ExecutionGraph, compile_workflow};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Prereqs / gating
// ---------------------------------------------------------------------------

fn e2e_enabled() -> bool {
    std::env::var("RUNTARA_RUN_COMPONENTS_E2E").as_deref() == Ok("1")
}

fn tool_installed(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn cargo_component_installed() -> bool {
    Command::new("cargo")
        .arg("component")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn agent_wasm_staged() -> bool {
    let dir = std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace_root().join("target/wasm32-wasip2/release"));
    dir.join("runtara_agent_crypto.wasm").exists()
}

fn setup_shared_data_dir() -> Option<TempDir> {
    // Caller-set DATA_DIR wins so debugging can keep the build tree.
    if std::env::var_os("DATA_DIR").is_some() {
        return None;
    }
    let temp = TempDir::new().expect("tempdir");
    // SAFETY: the smoke test binary is single-threaded (#[test] fn is one).
    unsafe {
        std::env::set_var("DATA_DIR", temp.path());
        // Shared target dir across every workflow this test compiles —
        // first compile pays the cold-build cost, every subsequent shape
        // is incremental.
        std::env::set_var(
            "RUNTARA_COMPONENTS_TARGET_DIR",
            temp.path().join("shared-target"),
        );
    }
    Some(temp)
}

// ---------------------------------------------------------------------------
// Workflow fixtures — one per shape we need to cover. Each is the minimum
// valid graph that exercises that DSL construct end-to-end.
// ---------------------------------------------------------------------------

/// A child workflow needed by an `EmbedWorkflow` step: (step_id, workflow_id,
/// child graph JSON). Empty for fixtures that don't embed.
type Children = &'static [(&'static str, &'static str, &'static str)];

struct Fixture {
    name: &'static str,
    json: &'static str,
    children: Children,
}

fn fixtures() -> Vec<Fixture> {
    let mut out: Vec<Fixture> = simple_fixtures()
        .into_iter()
        .map(|(name, json)| Fixture {
            name,
            json,
            children: &[],
        })
        .collect();

    // EmbedWorkflow needs a separately-loaded child graph.
    out.push(Fixture {
        name: "embed_workflow",
        json: r#"{
            "name": "embed_parent",
            "steps": {
                "ew": {
                    "stepType": "EmbedWorkflow",
                    "id": "ew",
                    "childWorkflowId": "smoke_child",
                    "childVersion": "latest",
                    "inputMapping": {
                        "v": {"valueType": "immediate", "value": "hi"}
                    }
                },
                "f": {
                    "stepType": "Finish",
                    "id": "f",
                    "inputMapping": {
                        "out": {"valueType": "reference", "value": "steps.ew.outputs"}
                    }
                }
            },
            "entryPoint": "ew",
            "executionPlan": [{"fromStep": "ew", "toStep": "f"}],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }"#,
        children: &[(
            "ew",
            "smoke_child",
            r#"{
                "name": "embed_child",
                "steps": {
                    "cf": {
                        "stepType": "Finish",
                        "id": "cf",
                        "inputMapping": {
                            "echo": {"valueType": "reference", "value": "data.v"}
                        }
                    }
                },
                "entryPoint": "cf",
                "executionPlan": [],
                "variables": {},
                "inputSchema": {"v": {"type": "string"}},
                "outputSchema": {}
            }"#,
        )],
    });

    out
}

fn simple_fixtures() -> Vec<(&'static str, &'static str)> {
    vec![
        // Trivial: only a Finish step.
        (
            "finish_only",
            r#"{
                "name": "finish_only",
                "steps": {
                    "f": {
                        "stepType": "Finish",
                        "id": "f",
                        "inputMapping": {
                            "x": {"valueType": "immediate", "value": "ok"}
                        }
                    }
                },
                "entryPoint": "f",
                "executionPlan": [],
                "variables": {},
                "inputSchema": {},
                "outputSchema": {}
            }"#,
        ),
        // Single Agent step (crypto/hash) feeding into Finish.
        (
            "agent_single",
            r#"{
                "name": "agent_single",
                "steps": {
                    "h": {
                        "stepType": "Agent",
                        "id": "h",
                        "agentId": "crypto",
                        "capabilityId": "hash",
                        "inputMapping": {
                            "data": {"valueType": "immediate", "value": "hello"},
                            "algorithm": {"valueType": "immediate", "value": "sha256"},
                            "output_format": {"valueType": "immediate", "value": "hex"}
                        }
                    },
                    "f": {
                        "stepType": "Finish",
                        "id": "f",
                        "inputMapping": {
                            "hash": {"valueType": "reference", "value": "steps.h.outputs.hash"}
                        }
                    }
                },
                "entryPoint": "h",
                "executionPlan": [{"fromStep": "h", "toStep": "f"}],
                "variables": {},
                "inputSchema": {},
                "outputSchema": {}
            }"#,
        ),
        // Two agents in the same workflow — verifies wac composition with
        // multiple per-agent imports.
        (
            "agent_multi",
            r#"{
                "name": "agent_multi",
                "steps": {
                    "h": {
                        "stepType": "Agent",
                        "id": "h",
                        "agentId": "crypto",
                        "capabilityId": "hash",
                        "inputMapping": {
                            "data": {"valueType": "immediate", "value": "hello"}
                        }
                    },
                    "t": {
                        "stepType": "Agent",
                        "id": "t",
                        "agentId": "transform",
                        "capabilityId": "extract",
                        "inputMapping": {
                            "value": {"valueType": "reference", "value": "steps.h.outputs"},
                            "path": {"valueType": "immediate", "value": "hash"}
                        }
                    },
                    "f": {
                        "stepType": "Finish",
                        "id": "f",
                        "inputMapping": {
                            "result": {"valueType": "reference", "value": "steps.t.outputs"}
                        }
                    }
                },
                "entryPoint": "h",
                "executionPlan": [
                    {"fromStep": "h", "toStep": "t"},
                    {"fromStep": "t", "toStep": "f"}
                ],
                "variables": {},
                "inputSchema": {},
                "outputSchema": {}
            }"#,
        ),
        // Split — fan-out / fan-in over a constant array.
        (
            "split",
            r#"{
                "name": "split",
                "steps": {
                    "s": {
                        "stepType": "Split",
                        "id": "s",
                        "config": {
                            "value": {"valueType": "immediate", "value": [1]}
                        },
                        "subgraph": {
                            "name": "row",
                            "steps": {
                                "rf": {
                                    "stepType": "Finish",
                                    "id": "rf",
                                    "inputMapping": {
                                        "x": {"valueType": "immediate", "value": "ok"}
                                    }
                                }
                            },
                            "entryPoint": "rf",
                            "executionPlan": []
                        }
                    },
                    "f": {
                        "stepType": "Finish",
                        "id": "f",
                        "inputMapping": {
                            "rows": {"valueType": "reference", "value": "steps.s.outputs"}
                        }
                    }
                },
                "entryPoint": "s",
                "executionPlan": [{"fromStep": "s", "toStep": "f"}],
                "variables": {},
                "inputSchema": {},
                "outputSchema": {}
            }"#,
        ),
        // Conditional — branch on a literal boolean.
        (
            "conditional",
            r#"{
                "name": "conditional",
                "steps": {
                    "c": {
                        "stepType": "Conditional",
                        "id": "c",
                        "condition": {
                            "type": "operation",
                            "op": "EQ",
                            "arguments": [
                                {"valueType": "immediate", "value": 1},
                                {"valueType": "immediate", "value": 1}
                            ]
                        }
                    },
                    "yes": {
                        "stepType": "Finish",
                        "id": "yes",
                        "inputMapping": {"x": {"valueType": "immediate", "value": "yes"}}
                    },
                    "no": {
                        "stepType": "Finish",
                        "id": "no",
                        "inputMapping": {"x": {"valueType": "immediate", "value": "no"}}
                    }
                },
                "entryPoint": "c",
                "executionPlan": [
                    {"fromStep": "c", "toStep": "yes", "label": "true"},
                    {"fromStep": "c", "toStep": "no", "label": "false"}
                ],
                "variables": {},
                "inputSchema": {},
                "outputSchema": {}
            }"#,
        ),
        // Switch — multi-way branch on a literal.
        (
            "switch",
            r#"{
                "name": "switch",
                "steps": {
                    "sw": {
                        "stepType": "Switch",
                        "id": "sw",
                        "config": {
                            "value": {"valueType": "immediate", "value": "a"},
                            "cases": [
                                {"matchType": "EQ", "match": "a", "output": "matched-a"},
                                {"matchType": "EQ", "match": "b", "output": "matched-b"}
                            ],
                            "default": "no-match"
                        }
                    },
                    "f": {
                        "stepType": "Finish",
                        "id": "f",
                        "inputMapping": {
                            "r": {"valueType": "reference", "value": "steps.sw.outputs"}
                        }
                    }
                },
                "entryPoint": "sw",
                "executionPlan": [{"fromStep": "sw", "toStep": "f"}],
                "variables": {},
                "inputSchema": {},
                "outputSchema": {}
            }"#,
        ),
        // While loop — terminates after one iteration (literal-false condition).
        (
            "while_loop",
            r#"{
                "name": "while_loop",
                "steps": {
                    "w": {
                        "stepType": "While",
                        "id": "w",
                        "condition": {
                            "type": "operation",
                            "op": "EQ",
                            "arguments": [
                                {"valueType": "immediate", "value": 1},
                                {"valueType": "immediate", "value": 2}
                            ]
                        },
                        "subgraph": {
                            "name": "body",
                            "steps": {
                                "wf": {
                                    "stepType": "Finish",
                                    "id": "wf",
                                    "inputMapping": {
                                        "x": {"valueType": "immediate", "value": "iter"}
                                    }
                                }
                            },
                            "entryPoint": "wf",
                            "executionPlan": []
                        },
                        "config": {"maxIterations": 1}
                    },
                    "f": {
                        "stepType": "Finish",
                        "id": "f",
                        "inputMapping": {
                            "iters": {"valueType": "reference", "value": "steps.w.outputs"}
                        }
                    }
                },
                "entryPoint": "w",
                "executionPlan": [{"fromStep": "w", "toStep": "f"}],
                "variables": {},
                "inputSchema": {},
                "outputSchema": {}
            }"#,
        ),
        // Log step — fields are top-level, not inputMapping.
        (
            "log",
            r#"{
                "name": "log",
                "steps": {
                    "l": {
                        "stepType": "Log",
                        "id": "l",
                        "level": "info",
                        "message": "smoke hello"
                    },
                    "f": {
                        "stepType": "Finish",
                        "id": "f",
                        "inputMapping": {"x": {"valueType": "immediate", "value": "done"}}
                    }
                },
                "entryPoint": "l",
                "executionPlan": [{"fromStep": "l", "toStep": "f"}],
                "variables": {},
                "inputSchema": {},
                "outputSchema": {}
            }"#,
        ),
        // Error step — top-level category/code/message/severity, gated by a
        // false branch so the workflow has a non-error happy path too.
        (
            "error",
            r#"{
                "name": "error",
                "steps": {
                    "c": {
                        "stepType": "Conditional",
                        "id": "c",
                        "condition": {
                            "type": "operation",
                            "op": "EQ",
                            "arguments": [
                                {"valueType": "immediate", "value": 1},
                                {"valueType": "immediate", "value": 2}
                            ]
                        }
                    },
                    "err": {
                        "stepType": "Error",
                        "id": "err",
                        "category": "permanent",
                        "code": "SMOKE_ERR",
                        "message": "smoke error",
                        "severity": "error"
                    },
                    "f": {
                        "stepType": "Finish",
                        "id": "f",
                        "inputMapping": {"x": {"valueType": "immediate", "value": "done"}}
                    }
                },
                "entryPoint": "c",
                "executionPlan": [
                    {"fromStep": "c", "toStep": "f", "label": "true"},
                    {"fromStep": "c", "toStep": "err", "label": "false"}
                ],
                "variables": {},
                "inputSchema": {},
                "outputSchema": {}
            }"#,
        ),
        // Filter — uses ConditionExpression on each element.
        (
            "filter",
            r#"{
                "name": "filter",
                "steps": {
                    "fi": {
                        "stepType": "Filter",
                        "id": "fi",
                        "config": {
                            "value": {"valueType": "immediate", "value": [{"k": 1}, {"k": 2}]},
                            "condition": {
                                "type": "operation",
                                "op": "EQ",
                                "arguments": [
                                    {"valueType": "reference", "value": "item.k"},
                                    {"valueType": "immediate", "value": 1}
                                ]
                            }
                        }
                    },
                    "f": {
                        "stepType": "Finish",
                        "id": "f",
                        "inputMapping": {
                            "filtered": {"valueType": "reference", "value": "steps.fi.outputs.items"}
                        }
                    }
                },
                "entryPoint": "fi",
                "executionPlan": [{"fromStep": "fi", "toStep": "f"}],
                "variables": {},
                "inputSchema": {},
                "outputSchema": {}
            }"#,
        ),
        // GroupBy — group an array by a field.
        (
            "group_by",
            r#"{
                "name": "group_by",
                "steps": {
                    "g": {
                        "stepType": "GroupBy",
                        "id": "g",
                        "config": {
                            "value": {"valueType": "immediate", "value": [{"s": "a"}, {"s": "b"}]},
                            "key": "s"
                        }
                    },
                    "f": {
                        "stepType": "Finish",
                        "id": "f",
                        "inputMapping": {
                            "groups": {"valueType": "reference", "value": "steps.g.outputs.groups"}
                        }
                    }
                },
                "entryPoint": "g",
                "executionPlan": [{"fromStep": "g", "toStep": "f"}],
                "variables": {},
                "inputSchema": {},
                "outputSchema": {}
            }"#,
        ),
        // Delay — short non-blocking sleep.
        (
            "delay",
            r#"{
                "name": "delay",
                "steps": {
                    "d": {
                        "stepType": "Delay",
                        "id": "d",
                        "durationMs": {"valueType": "immediate", "value": 1}
                    },
                    "f": {
                        "stepType": "Finish",
                        "id": "f",
                        "inputMapping": {"x": {"valueType": "immediate", "value": "done"}}
                    }
                },
                "entryPoint": "d",
                "executionPlan": [{"fromStep": "d", "toStep": "f"}],
                "variables": {},
                "inputSchema": {},
                "outputSchema": {}
            }"#,
        ),
        // WaitForSignal — durable wait construct (compile-only here).
        (
            "wait_for_signal",
            r#"{
                "name": "wait_for_signal",
                "steps": {
                    "w": {
                        "stepType": "WaitForSignal",
                        "id": "w",
                        "timeoutMs": {"valueType": "immediate", "value": 30000},
                        "pollIntervalMs": 500
                    },
                    "f": {
                        "stepType": "Finish",
                        "id": "f",
                        "inputMapping": {
                            "sid": {"valueType": "reference", "value": "steps.w.signal_id"}
                        }
                    }
                },
                "entryPoint": "w",
                "executionPlan": [{"fromStep": "w", "toStep": "f"}],
                "variables": {},
                "inputSchema": {},
                "outputSchema": {}
            }"#,
        ),
    ]
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[test]
fn components_smoke_all_shapes() {
    if !e2e_enabled() {
        eprintln!(
            "SKIP: components_smoke — set RUNTARA_RUN_COMPONENTS_E2E=1 to run \
             (heavy: cold cargo-component build ~30s, warm ~3s per shape)."
        );
        return;
    }
    if !cargo_component_installed() {
        eprintln!("SKIP: cargo-component not installed.");
        return;
    }
    if !tool_installed("wac") {
        eprintln!("SKIP: wac not installed.");
        return;
    }
    if !agent_wasm_staged() {
        eprintln!("SKIP: agent components not staged. Run scripts/build-agent-components.sh.");
        return;
    }

    let _shared = setup_shared_data_dir();

    let mut failed: Vec<(String, String)> = Vec::new();
    let fixtures = fixtures();
    let total = fixtures.len();
    for (i, fx) in fixtures.into_iter().enumerate() {
        let Fixture {
            name,
            json,
            children,
        } = fx;

        let graph: ExecutionGraph = match serde_json::from_str(json) {
            Ok(g) => g,
            Err(e) => {
                failed.push((name.into(), format!("fixture JSON invalid: {e}")));
                continue;
            }
        };

        let mut child_workflows = Vec::with_capacity(children.len());
        let mut child_parse_err: Option<String> = None;
        for (step_id, workflow_id, child_json) in children {
            match serde_json::from_str::<ExecutionGraph>(child_json) {
                Ok(child_graph) => child_workflows.push(ChildWorkflowInput {
                    step_id: (*step_id).to_string(),
                    workflow_id: (*workflow_id).to_string(),
                    version_requested: "latest".to_string(),
                    version_resolved: 1,
                    execution_graph: child_graph,
                }),
                Err(e) => {
                    child_parse_err = Some(format!("child {workflow_id} JSON invalid: {e}"));
                    break;
                }
            }
        }
        if let Some(err) = child_parse_err {
            failed.push((name.into(), err));
            continue;
        }

        let input = CompilationInput {
            tenant_id: "components-smoke".to_string(),
            workflow_id: format!("components_smoke_{name}"),
            version: 1,
            execution_graph: graph,
            track_events: false,
            child_workflows,
            connection_service_url: None,
            connection_integration_ids: std::collections::HashMap::new(),
            agent_catalog: None,
            progress_callback: None,
        };

        let started = std::time::Instant::now();
        match compile_workflow(input) {
            Ok(result) => {
                let elapsed = started.elapsed().as_secs_f64();
                eprintln!(
                    "✓ [{i:>2}/{total}] {name:<20} {bytes:>10} bytes  ({elapsed:.1}s)",
                    bytes = result.binary_size,
                );
                assert!(
                    result.binary_path.exists(),
                    "{name}: binary path does not exist"
                );
                assert!(result.binary_size > 0, "{name}: zero-byte binary");
            }
            Err(e) => {
                let elapsed = started.elapsed().as_secs_f64();
                eprintln!("✗ [{i:>2}/{total}] {name:<20} FAILED ({elapsed:.1}s): {e}");
                failed.push((name.into(), e.to_string()));
            }
        }
    }

    if !failed.is_empty() {
        let summary: String = failed
            .iter()
            .map(|(name, err)| format!("  • {name}: {err}\n"))
            .collect();
        panic!(
            "{} of {} shapes failed components-mode compilation:\n{}",
            failed.len(),
            total,
            summary
        );
    }
}
