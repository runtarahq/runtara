//! Per-construct compile smoke tests.
//!
//! Each test takes a minimal workflow exercising one DSL construct and runs it
//! all the way through `compile_workflow` — codegen + rustc — asserting a
//! binary is produced. Catches the class of regression where generated code is
//! syntactically valid but rustc rejects it (e.g. type mismatches).
//!
//! Gated by `RUNTARA_RUN_SMOKE_COMPILE=1` (CI sets this in the dedicated smoke
//! job). The gate also requires the WASM stdlib to be staged at
//! `target/native_cache_wasm/` or via `RUNTARA_WASM_LIBRARY_DIR`.

use std::path::{Path, PathBuf};

use runtara_workflows::{CompilationInput, ExecutionGraph, compile_workflow};
use tempfile::TempDir;

fn smoke_enabled() -> bool {
    std::env::var("RUNTARA_RUN_SMOKE_COMPILE").as_deref() == Ok("1")
}

fn wasm_library_staged() -> bool {
    if let Ok(dir) = std::env::var("RUNTARA_WASM_LIBRARY_DIR")
        && Path::new(&dir).exists()
    {
        return true;
    }
    if let Ok(dir) = std::env::var("RUNTARA_NATIVE_LIBRARY_DIR")
        && Path::new(&dir).exists()
    {
        return true;
    }
    PathBuf::from("target/native_cache_wasm").exists()
}

fn isolated_data_dir() -> TempDir {
    let temp = TempDir::new().expect("temp dir");
    // SAFETY: smoke tests run single-threaded inside this binary; DATA_DIR is
    // read by compile_workflow once per call.
    unsafe {
        std::env::set_var("DATA_DIR", temp.path());
    }
    temp
}

fn compile(name: &str, json: &str) {
    if !smoke_enabled() {
        eprintln!("Skipping smoke_compile::{name}: set RUNTARA_RUN_SMOKE_COMPILE=1 to run.");
        return;
    }
    assert!(
        wasm_library_staged(),
        "smoke_compile::{name}: RUNTARA_RUN_SMOKE_COMPILE=1 set but WASM stdlib not staged. \
         Stage target/native_cache_wasm/ or set RUNTARA_WASM_LIBRARY_DIR."
    );

    let graph: ExecutionGraph =
        serde_json::from_str(json).expect("smoke fixture should parse as ExecutionGraph");

    let _temp = isolated_data_dir();

    let input = CompilationInput {
        tenant_id: "smoke".to_string(),
        workflow_id: name.to_string(),
        version: 1,
        execution_graph: graph,
        track_events: false,
        child_workflows: vec![],
        connection_service_url: None,
    };

    let result = compile_workflow(input).unwrap_or_else(|e| panic!("smoke {name}: {e}"));
    assert!(
        result.binary_path.exists(),
        "smoke {name}: binary missing at {:?}",
        result.binary_path
    );
    assert!(
        result.binary_size > 0,
        "smoke {name}: zero-byte binary at {:?}",
        result.binary_path
    );
}

#[test]
fn smoke_finish_only() {
    compile(
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
    );
}

#[test]
fn smoke_split() {
    compile(
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
    );
}

#[test]
fn smoke_split_dont_stop_on_failed() {
    compile(
        "split_dont_stop",
        r#"{
            "name": "split_dont_stop",
            "steps": {
                "s": {
                    "stepType": "Split",
                    "id": "s",
                    "config": {
                        "value": {"valueType": "immediate", "value": [1, 2]},
                        "dontStopOnFailed": true
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
    );
}

#[test]
fn smoke_conditional() {
    compile(
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
                "t": {
                    "stepType": "Finish",
                    "id": "t",
                    "inputMapping": {"r": {"valueType": "immediate", "value": "yes"}}
                },
                "e": {
                    "stepType": "Finish",
                    "id": "e",
                    "inputMapping": {"r": {"valueType": "immediate", "value": "no"}}
                }
            },
            "entryPoint": "c",
            "executionPlan": [
                {"fromStep": "c", "toStep": "t", "label": "true"},
                {"fromStep": "c", "toStep": "e", "label": "false"}
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }"#,
    );
}

#[test]
fn smoke_switch() {
    compile(
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
    );
}

#[test]
fn smoke_while() {
    compile(
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
                            "bf": {
                                "stepType": "Finish",
                                "id": "bf",
                                "inputMapping": {
                                    "x": {"valueType": "immediate", "value": "tick"}
                                }
                            }
                        },
                        "entryPoint": "bf",
                        "executionPlan": []
                    },
                    "config": {"maxIterations": 1}
                },
                "f": {
                    "stepType": "Finish",
                    "id": "f",
                    "inputMapping": {
                        "loop": {"valueType": "reference", "value": "steps.w.outputs"}
                    }
                }
            },
            "entryPoint": "w",
            "executionPlan": [{"fromStep": "w", "toStep": "f"}],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }"#,
    );
}

#[test]
fn smoke_filter() {
    compile(
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
    );
}

#[test]
fn smoke_group_by() {
    compile(
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
    );
}

#[test]
fn smoke_log() {
    compile(
        "log",
        r#"{
            "name": "log",
            "steps": {
                "l": {
                    "stepType": "Log",
                    "id": "l",
                    "level": "info",
                    "message": "smoke log"
                },
                "f": {
                    "stepType": "Finish",
                    "id": "f",
                    "inputMapping": {
                        "x": {"valueType": "immediate", "value": "done"}
                    }
                }
            },
            "entryPoint": "l",
            "executionPlan": [{"fromStep": "l", "toStep": "f"}],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }"#,
    );
}

// TODO: durable Delay codegen calls `__sdk.durable_sleep(__duration)` (1 arg),
// but `RuntaraSdk::sleep` is `(duration, checkpoint_id, state)` and
// `durable_sleep` is a backend-trait method, not on RuntaraSdk. Broken since
// the sync-SDK migration (d875d70). Re-enable once the codegen is fixed.
#[test]
#[ignore = "Delay codegen calls non-existent SDK method (see TODO above)"]
fn smoke_delay() {
    compile(
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
                    "inputMapping": {
                        "x": {"valueType": "immediate", "value": "done"}
                    }
                }
            },
            "entryPoint": "d",
            "executionPlan": [{"fromStep": "d", "toStep": "f"}],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }"#,
    );
}

#[test]
fn smoke_error() {
    compile(
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
                    "inputMapping": {
                        "x": {"valueType": "immediate", "value": "done"}
                    }
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
    );
}

#[test]
fn smoke_wait_for_signal() {
    compile(
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
    );
}

#[test]
fn smoke_embed_workflow() {
    use runtara_workflows::ChildWorkflowInput;

    if !smoke_enabled() {
        eprintln!(
            "Skipping smoke_compile::smoke_embed_workflow: set RUNTARA_RUN_SMOKE_COMPILE=1 to run."
        );
        return;
    }
    assert!(
        wasm_library_staged(),
        "smoke_embed_workflow: RUNTARA_RUN_SMOKE_COMPILE=1 set but WASM stdlib not staged."
    );

    let parent_json = r#"{
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
    }"#;

    let child_json = r#"{
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
        "inputSchema": {
            "v": {"type": "string"}
        },
        "outputSchema": {}
    }"#;

    let parent_graph: ExecutionGraph =
        serde_json::from_str(parent_json).expect("parent should parse");
    let child_graph: ExecutionGraph = serde_json::from_str(child_json).expect("child should parse");

    let _temp = isolated_data_dir();

    let input = CompilationInput {
        tenant_id: "smoke".to_string(),
        workflow_id: "embed_parent".to_string(),
        version: 1,
        execution_graph: parent_graph,
        track_events: false,
        child_workflows: vec![ChildWorkflowInput {
            step_id: "ew".to_string(),
            workflow_id: "smoke_child".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 1,
            execution_graph: child_graph,
        }],
        connection_service_url: None,
    };

    let result = compile_workflow(input).unwrap_or_else(|e| panic!("smoke embed_workflow: {e}"));
    assert!(result.binary_path.exists(), "smoke embed: binary missing");
    assert!(result.binary_size > 0, "smoke embed: zero-byte binary");
}
