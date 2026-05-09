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

#[derive(Default)]
struct CompileOpts {
    track_events: bool,
}

fn compile(name: &str, json: &str) {
    compile_with_opts(name, json, CompileOpts::default());
}

fn compile_with_opts(name: &str, json: &str, opts: CompileOpts) {
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
        track_events: opts.track_events,
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

/// Split with populated `config.variables` containing a reference to a
/// data field. Triggers the codegen path where `mapping::emit_input_mapping`
/// is interpolated *inside* a block that locally rebinds `inputs` to a
/// `serde_json::Value` — which collides with the helper's emission of
/// `inputs.as_ref()` against the (no-longer) `Arc<WorkflowInputs>`. Without
/// the rename fix, this fixture fails compile with E0599 on `as_ref` for
/// `serde_json::Value`.
#[test]
fn smoke_split_with_variables() {
    compile(
        "split_with_variables",
        r#"{
            "name": "split_with_variables",
            "steps": {
                "s": {
                    "stepType": "Split",
                    "id": "s",
                    "config": {
                        "value": {"valueType": "immediate", "value": [1]},
                        "variables": {
                            "tag": {"valueType": "reference", "value": "data.label"}
                        }
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
            "inputSchema": {
                "label": {"type": "string"}
            },
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

#[test]
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

// ============================================================================
// Tier 1 — config variants that hit different codegen branches
// ============================================================================

/// Parallel Split (parallelism > 1) — exercises the std::thread::scope codegen
/// path, distinct from the sequential `for` loop used in default smoke_split.
#[test]
fn smoke_split_parallel() {
    compile(
        "split_parallel",
        r#"{
            "name": "split_parallel",
            "steps": {
                "s": {
                    "stepType": "Split",
                    "id": "s",
                    "config": {
                        "value": {"valueType": "immediate", "value": [1, 2, 3]},
                        "parallelism": 2
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

/// Split with batchSize — exercises chunking logic that groups iterations
/// into fixed-size batches before dispatching to the subgraph.
#[test]
fn smoke_split_batch_size() {
    compile(
        "split_batch_size",
        r#"{
            "name": "split_batch_size",
            "steps": {
                "s": {
                    "stepType": "Split",
                    "id": "s",
                    "config": {
                        "value": {"valueType": "immediate", "value": [1, 2, 3, 4, 5]},
                        "batchSize": 2
                    },
                    "subgraph": {
                        "name": "batch",
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

/// Routing Switch — when cases carry `route:` labels, the Switch becomes a
/// branching control-flow step rather than a value producer. Distinct codegen
/// path from default smoke_switch.
#[test]
fn smoke_switch_routing() {
    compile(
        "switch_routing",
        r#"{
            "name": "switch_routing",
            "steps": {
                "sw": {
                    "stepType": "Switch",
                    "id": "sw",
                    "config": {
                        "value": {"valueType": "immediate", "value": "a"},
                        "cases": [
                            {"matchType": "EQ", "match": "a", "output": "matched-a", "route": "route_a"},
                            {"matchType": "EQ", "match": "b", "output": "matched-b", "route": "route_b"}
                        ],
                        "default": "no-match"
                    }
                },
                "fa": {
                    "stepType": "Finish",
                    "id": "fa",
                    "inputMapping": {"r": {"valueType": "immediate", "value": "took-a"}}
                },
                "fb": {
                    "stepType": "Finish",
                    "id": "fb",
                    "inputMapping": {"r": {"valueType": "immediate", "value": "took-b"}}
                },
                "fd": {
                    "stepType": "Finish",
                    "id": "fd",
                    "inputMapping": {"r": {"valueType": "immediate", "value": "took-default"}}
                }
            },
            "entryPoint": "sw",
            "executionPlan": [
                {"fromStep": "sw", "toStep": "fa", "label": "route_a"},
                {"fromStep": "sw", "toStep": "fb", "label": "route_b"},
                {"fromStep": "sw", "toStep": "fd", "label": "default"}
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }"#,
    );
}

/// While whose condition references `loop.index` (a special root only valid
/// in While conditions) with an explicit `maxIterations` cap. Exercises the
/// iteration-counter wiring distinct from default smoke_while which uses
/// only immediate-vs-immediate comparison.
#[test]
fn smoke_while_max_iter_loop_index() {
    compile(
        "while_max_iter_loop_index",
        r#"{
            "name": "while_max_iter_loop_index",
            "steps": {
                "w": {
                    "stepType": "While",
                    "id": "w",
                    "condition": {
                        "type": "operation",
                        "op": "LT",
                        "arguments": [
                            {"valueType": "reference", "value": "loop.index"},
                            {"valueType": "immediate", "value": 3}
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
                    "config": {"maxIterations": 3}
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

/// Error step with `category: transient` and a `context` mapping. Exercises
/// the transient branch of category emission and the context-mapping path
/// (default smoke_error uses `permanent` and no context).
#[test]
fn smoke_error_transient_context() {
    compile(
        "error_transient_context",
        r#"{
            "name": "error_transient_context",
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
                    "category": "transient",
                    "code": "SMOKE_TRANSIENT",
                    "message": "smoke transient error",
                    "severity": "warning",
                    "context": {
                        "attempt_id": {"valueType": "immediate", "value": "abc-123"},
                        "endpoint": {"valueType": "immediate", "value": "/v1/widgets"}
                    }
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

/// Delay where `durationMs` is a reference (not an immediate literal).
/// Exercises the mapping-evaluation path; default smoke_delay uses an
/// immediate.
#[test]
fn smoke_delay_dynamic() {
    compile(
        "delay_dynamic",
        r#"{
            "name": "delay_dynamic",
            "steps": {
                "d": {
                    "stepType": "Delay",
                    "id": "d",
                    "durationMs": {"valueType": "reference", "value": "data.wait_ms"}
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
            "inputSchema": {
                "wait_ms": {"type": "integer"}
            },
            "outputSchema": {}
        }"#,
    );
}

/// Conditional with a composite AND-of-OR expression. Exercises the
/// recursive condition emitter; default smoke_conditional has only a single
/// EQ comparison.
#[test]
fn smoke_conditional_and_or() {
    compile(
        "conditional_and_or",
        r#"{
            "name": "conditional_and_or",
            "steps": {
                "c": {
                    "stepType": "Conditional",
                    "id": "c",
                    "condition": {
                        "type": "operation",
                        "op": "AND",
                        "arguments": [
                            {
                                "type": "operation",
                                "op": "OR",
                                "arguments": [
                                    {
                                        "type": "operation",
                                        "op": "EQ",
                                        "arguments": [
                                            {"valueType": "immediate", "value": 1},
                                            {"valueType": "immediate", "value": 1}
                                        ]
                                    },
                                    {
                                        "type": "operation",
                                        "op": "EQ",
                                        "arguments": [
                                            {"valueType": "immediate", "value": "x"},
                                            {"valueType": "immediate", "value": "y"}
                                        ]
                                    }
                                ]
                            },
                            {
                                "type": "operation",
                                "op": "GT",
                                "arguments": [
                                    {"valueType": "immediate", "value": 5},
                                    {"valueType": "immediate", "value": 3}
                                ]
                            }
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

// ============================================================================
// Tier 2 — nested combinations (subgraph-in-subgraph plumbing)
// ============================================================================

/// Split inside While — two levels of subgraph nesting. Exercises scope-id
/// propagation and variable inheritance through nested iteration scopes.
#[test]
fn smoke_split_in_while() {
    compile(
        "split_in_while",
        r#"{
            "name": "split_in_while",
            "steps": {
                "w": {
                    "stepType": "While",
                    "id": "w",
                    "condition": {
                        "type": "operation",
                        "op": "LT",
                        "arguments": [
                            {"valueType": "reference", "value": "loop.index"},
                            {"valueType": "immediate", "value": 2}
                        ]
                    },
                    "subgraph": {
                        "name": "outer",
                        "steps": {
                            "s": {
                                "stepType": "Split",
                                "id": "s",
                                "config": {
                                    "value": {"valueType": "immediate", "value": [1, 2]}
                                },
                                "subgraph": {
                                    "name": "inner",
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
                            "wf": {
                                "stepType": "Finish",
                                "id": "wf",
                                "inputMapping": {
                                    "rows": {"valueType": "reference", "value": "steps.s.outputs"}
                                }
                            }
                        },
                        "entryPoint": "s",
                        "executionPlan": [{"fromStep": "s", "toStep": "wf"}]
                    },
                    "config": {"maxIterations": 2}
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

/// Conditional inside Split — branching control flow inside an iteration.
/// Both branches converge on a single Finish so the subgraph has a unique
/// terminal step. Exercises branch emission inside iteration scope.
#[test]
fn smoke_conditional_in_split() {
    compile(
        "conditional_in_split",
        r#"{
            "name": "conditional_in_split",
            "steps": {
                "s": {
                    "stepType": "Split",
                    "id": "s",
                    "config": {
                        "value": {"valueType": "immediate", "value": [1, 2]}
                    },
                    "subgraph": {
                        "name": "row",
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
                            "rf": {
                                "stepType": "Finish",
                                "id": "rf",
                                "inputMapping": {
                                    "x": {"valueType": "immediate", "value": "ok"}
                                }
                            }
                        },
                        "entryPoint": "c",
                        "executionPlan": [
                            {"fromStep": "c", "toStep": "rf", "label": "true"},
                            {"fromStep": "c", "toStep": "rf", "label": "false"}
                        ]
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

/// Three-level EmbedWorkflow chain (parent → child → grandchild). Exercises
/// child_workflows resolution at more than one level of nesting and verifies
/// the codegen correctly threads inputs/outputs through embedded boundaries.
#[test]
fn smoke_embed_chain_3_levels() {
    use runtara_workflows::ChildWorkflowInput;

    if !smoke_enabled() {
        eprintln!(
            "Skipping smoke_compile::smoke_embed_chain_3_levels: set RUNTARA_RUN_SMOKE_COMPILE=1 to run."
        );
        return;
    }
    assert!(
        wasm_library_staged(),
        "smoke_embed_chain_3_levels: RUNTARA_RUN_SMOKE_COMPILE=1 set but WASM stdlib not staged."
    );

    let parent_json = r#"{
        "name": "embed_parent_3lvl",
        "steps": {
            "ew": {
                "stepType": "EmbedWorkflow",
                "id": "ew",
                "childWorkflowId": "smoke_child_lvl1",
                "childVersion": "latest",
                "inputMapping": {
                    "v": {"valueType": "immediate", "value": "parent-value"}
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

    let child_lvl1_json = r#"{
        "name": "embed_child_lvl1",
        "steps": {
            "ew2": {
                "stepType": "EmbedWorkflow",
                "id": "ew2",
                "childWorkflowId": "smoke_child_lvl2",
                "childVersion": "latest",
                "inputMapping": {
                    "v": {"valueType": "reference", "value": "data.v"}
                }
            },
            "cf1": {
                "stepType": "Finish",
                "id": "cf1",
                "inputMapping": {
                    "lvl1_out": {"valueType": "reference", "value": "steps.ew2.outputs"}
                }
            }
        },
        "entryPoint": "ew2",
        "executionPlan": [{"fromStep": "ew2", "toStep": "cf1"}],
        "variables": {},
        "inputSchema": {
            "v": {"type": "string"}
        },
        "outputSchema": {}
    }"#;

    let child_lvl2_json = r#"{
        "name": "embed_child_lvl2",
        "steps": {
            "cf2": {
                "stepType": "Finish",
                "id": "cf2",
                "inputMapping": {
                    "echo": {"valueType": "reference", "value": "data.v"}
                }
            }
        },
        "entryPoint": "cf2",
        "executionPlan": [],
        "variables": {},
        "inputSchema": {
            "v": {"type": "string"}
        },
        "outputSchema": {}
    }"#;

    let parent_graph: ExecutionGraph = serde_json::from_str(parent_json).expect("parent parses");
    let child_lvl1_graph: ExecutionGraph =
        serde_json::from_str(child_lvl1_json).expect("child lvl1 parses");
    let child_lvl2_graph: ExecutionGraph =
        serde_json::from_str(child_lvl2_json).expect("child lvl2 parses");

    let _temp = isolated_data_dir();

    let input = CompilationInput {
        tenant_id: "smoke".to_string(),
        workflow_id: "embed_parent_3lvl".to_string(),
        version: 1,
        execution_graph: parent_graph,
        track_events: false,
        child_workflows: vec![
            ChildWorkflowInput {
                step_id: "ew".to_string(),
                workflow_id: "smoke_child_lvl1".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 1,
                execution_graph: child_lvl1_graph,
            },
            ChildWorkflowInput {
                step_id: "ew2".to_string(),
                workflow_id: "smoke_child_lvl2".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 1,
                execution_graph: child_lvl2_graph,
            },
        ],
        connection_service_url: None,
    };

    let result =
        compile_workflow(input).unwrap_or_else(|e| panic!("smoke embed_chain_3_levels: {e}"));
    assert!(
        result.binary_path.exists(),
        "smoke embed 3lvl: binary missing"
    );
    assert!(result.binary_size > 0, "smoke embed 3lvl: zero-byte binary");
}

// ============================================================================
// Tier 3 — orthogonal axes (cross-cutting codegen)
// ============================================================================

/// Workflow with `variables` defined at the workflow level. A step references
/// a workflow variable via `variables.<name>`. Exercises the variable codegen
/// path that injects workflow variables into the runtime context.
#[test]
fn smoke_workflow_variables() {
    compile(
        "workflow_variables",
        r#"{
            "name": "workflow_variables",
            "steps": {
                "f": {
                    "stepType": "Finish",
                    "id": "f",
                    "inputMapping": {
                        "greeting": {"valueType": "reference", "value": "variables.greeting"},
                        "count": {"valueType": "reference", "value": "variables.count"}
                    }
                }
            },
            "entryPoint": "f",
            "executionPlan": [],
            "variables": {
                "greeting": {"type": "string", "value": "hello"},
                "count": {"type": "integer", "value": 42}
            },
            "inputSchema": {},
            "outputSchema": {}
        }"#,
    );
}

/// Workflow with non-empty inputSchema covering multiple field types
/// (string, integer, boolean, array) plus a non-empty outputSchema. Exercises
/// the schema-validator emission and the `data.*` reference path.
#[test]
fn smoke_non_empty_schemas() {
    compile(
        "non_empty_schemas",
        r#"{
            "name": "non_empty_schemas",
            "steps": {
                "f": {
                    "stepType": "Finish",
                    "id": "f",
                    "inputMapping": {
                        "name": {"valueType": "reference", "value": "data.name"},
                        "count": {"valueType": "reference", "value": "data.count"},
                        "active": {"valueType": "reference", "value": "data.active"},
                        "tags": {"valueType": "reference", "value": "data.tags"}
                    }
                }
            },
            "entryPoint": "f",
            "executionPlan": [],
            "variables": {},
            "inputSchema": {
                "name": {"type": "string", "required": true},
                "count": {"type": "integer"},
                "active": {"type": "boolean"},
                "tags": {"type": "array"}
            },
            "outputSchema": {
                "name": {"type": "string"},
                "count": {"type": "integer"},
                "active": {"type": "boolean"},
                "tags": {"type": "array"}
            }
        }"#,
    );
}

/// Workflow compiled with `track_events: true` on the CompilationInput.
/// Exercises the event-emission codegen branches that wrap each step in
/// custom event emitters.
#[test]
fn smoke_track_events() {
    compile_with_opts(
        "track_events",
        r#"{
            "name": "track_events",
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
        CompileOpts { track_events: true },
    );
}

/// Workflow with `durable: false` at the workflow level. Exercises the
/// non-durable code path: no `#[resilient]` macro, no checkpointing, no
/// `__sdk.sleep` for Delay (uses std::thread::sleep instead).
#[test]
fn smoke_workflow_non_durable() {
    compile(
        "workflow_non_durable",
        r#"{
            "name": "workflow_non_durable",
            "durable": false,
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
                        "x": {"valueType": "immediate", "value": "ok"}
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

/// Step with `breakpoint: true`. Exercises the breakpoint-check emission
/// path that pauses execution before the step in debug mode.
#[test]
fn smoke_step_breakpoint() {
    compile(
        "step_breakpoint",
        r#"{
            "name": "step_breakpoint",
            "steps": {
                "c": {
                    "stepType": "Conditional",
                    "id": "c",
                    "breakpoint": true,
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
