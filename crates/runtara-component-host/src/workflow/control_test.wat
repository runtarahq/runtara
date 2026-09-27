;; Fixtures for control_tests.rs. Sections are split on the `;;--` markers.
;;-- TYPES
  (type $error (record (field "code" string) (field "message" string)
    (field "category" string) (field "severity" string) (field "retryable" bool)
    (field "retry-after-ms" (option u64)) (field "attributes" (option string))))
  (type $wake (variant (case "at" u64) (case "instances" string)))
  (type $suspension (record (field "wakes" (list $wake)) (field "state" (list u8))))
  (type $outcome (variant (case "completed" (list u8)) (case "suspended" $suspension)))
;;-- EXECUTOR
  (import "runtara:control/executor@0.1.0" (instance $exec
    (type $error-def (record (field "code" string) (field "message" string)
      (field "category" string) (field "severity" string) (field "retryable" bool)
      (field "retry-after-ms" (option u64)) (field "attributes" (option string))))
    (export "error-info" (type $e (eq $error-def)))
    (type $wake-def (variant (case "at" u64) (case "instances" string)))
    (export "wake" (type $w (eq $wake-def)))
    (type $suspension-def (record (field "wakes" (list $w)) (field "state" (list u8))))
    (export "suspension" (type $s (eq $suspension-def)))
    (type $outcome-def (variant (case "completed" (list u8)) (case "suspended" $s)))
    (export "outcome" (type $o (eq $outcome-def)))
    (export "invoke" (func async (param "capability-id" string) (param "input" (list u8))
      (result (result $o (error $e)))))))
;;-- API
  (import "runtara:control/api@0.1.0" (instance $api
    (type $mode-def (enum "all" "any"))
    (export "wait-mode" (type $mode (eq $mode-def)))
    (type $status-def (enum "queued" "pending" "running" "suspended" "completed" "failed"
      "cancelled" "not-started"))
    (export "instance-status" (type $status (eq $status-def)))
    (type $terminal-def (record (field "output" (option (list u8)))
      (field "output-bytes" (option u64)) (field "output-omitted" bool)
      (field "error" (option (list u8))) (field "error-omitted" bool)))
    (export "terminal-result" (type $terminal (eq $terminal-def)))
    (type $target-def (record (field "instance-id" string) (field "status" $status)
      (field "finished-at-ms" (option u64)) (field "terminal" $terminal)))
    (export "target-outcome" (type $target (eq $target-def)))
    (type $progress-def (record (field "mode" $mode) (field "finished" (list $target))
      (field "remaining" (list string)) (field "deadline-ms" (option u64))))
    (export "wait-progress" (type $progress (eq $progress-def)))
    (type $resolution-def (enum "satisfied" "deadline" "empty"))
    (export "wait-resolution" (type $resolution (eq $resolution-def)))
    (type $settled-def (record (field "resolution" $resolution) (field "progress" $progress)))
    (export "wait-settled" (type $settled (eq $settled-def)))
    (type $poll-def (variant (case "pending" $progress) (case "settled" $settled)))
    (export "wait-poll" (type $poll (eq $poll-def)))
    (type $code-def (enum "denied" "invalid" "not-found" "not-runnable" "not-child"
      "requires-instance" "requires-operation" "capacity" "replay-conflict" "label-conflict"
      "too-large" "unavailable" "unsupported" "not-waiting" "ambiguous" "already-answered"
      "not-pausable" "not-paused" "wait-closed"))
    (export "error-code" (type $code (eq $code-def)))
    (type $cerror-def (record (field "code" $code) (field "message" string)
      (field "retry-after-ms" (option u64))))
    (export "control-error" (type $control-error (eq $cerror-def)))
    (export "poll-wait" (func async (param "wait-id" string)
      (result (result $poll (error $control-error)))))))
;;-- AGENT
  ;; The control agent fixture: imports the executor and the API, exports
  ;; `execution`. By the capability id's first byte: `p` polls the API and
  ;; answers "polled", `b` answers 4 MiB + 1 bytes, `s` suspends with a
  ;; 64 KiB + 1 continuation, anything else answers "ok".
  {{EXECUTOR}}
  {{API}}
  (alias export $api "poll-wait" (func $poll-wait))
  (core module $memory
    (memory (export "memory") 70)
    (global $heap (mut i32) (i32.const 8192))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32) (local $p i32)
      (local.set $p (i32.and (i32.add (global.get $heap) (i32.const 7)) (i32.const -8)))
      (global.set $heap (i32.add (local.get $p) (local.get 3)))
      (local.get $p)))
  (core instance $memory (instantiate $memory))
  (core func $poll (canon lower (func $poll-wait) (memory $memory "memory")
    (realloc (func $memory "realloc"))))
  (core module $code
    (import "m" "memory" (memory 70))
    (import "h" "poll" (func $poll (param i32 i32 i32)))
    (data (i32.const 1024) "w")
    (data (i32.const 1040) "\22polled\22")
    (data (i32.const 1056) "\22ok\22")
    (func (export "execute") (param i32 i32 i32 i32 i32 i32 i32) (result i32)
      (local $c i32)
      (local.set $c (i32.load8_u (local.get 0)))
      (i32.store8 (i32.const 2048) (i32.const 0))
      (if (i32.eq (local.get $c) (i32.const 112))
        (then
          (call $poll (i32.const 1024) (i32.const 1) (i32.const 3072))
          (i32.store8 (i32.const 2056) (i32.const 0))
          (i32.store (i32.const 2060) (i32.const 1040))
          (i32.store (i32.const 2064) (i32.const 8))
          (return (i32.const 2048))))
      (if (i32.eq (local.get $c) (i32.const 98))
        (then
          (i32.store8 (i32.const 2056) (i32.const 0))
          (i32.store (i32.const 2060) (i32.const 0))
          (i32.store (i32.const 2064) (i32.const 4194305))
          (return (i32.const 2048))))
      (if (i32.eq (local.get $c) (i32.const 115))
        (then
          (i32.store8 (i32.const 2056) (i32.const 1))
          (i32.store (i32.const 2060) (i32.const 0))
          (i32.store (i32.const 2064) (i32.const 0))
          (i32.store (i32.const 2068) (i32.const 0))
          (i32.store (i32.const 2072) (i32.const 65537))
          (return (i32.const 2048))))
      (i32.store8 (i32.const 2056) (i32.const 0))
      (i32.store (i32.const 2060) (i32.const 1056))
      (i32.store (i32.const 2064) (i32.const 4))
      (i32.const 2048)))
  (core instance $code (instantiate $code
    (with "m" (instance $memory))
    (with "h" (instance (export "poll" (func $poll))))))
  {{TYPES}}
  (func $execute async (param "capability-id" string) (param "input" (list u8))
    (param "continuation" (option (list u8)))
    (result (result $outcome (error $error)))
    (canon lift (core func $code "execute") (memory $memory "memory")
      (realloc (func $memory "realloc"))))
  (instance $execution (export "error-info" (type $error)) (export "wake" (type $wake))
    (export "suspension" (type $suspension)) (export "outcome" (type $outcome))
    (export "invoke" (func $execute)))
  (export "runtara:control/execution@0.1.0" (instance $execution))
;;-- ROOT
  ;; A workflow root: pins, imports control, nests the agent, and forwards
  ;; its input as the capability id to the executor.
  {{PINS}}
  {{EXECUTOR}}
  {{API}}
  {{NESTED}}
  (core module $rmem
    (memory (export "memory") 1)
    (global $heap (mut i32) (i32.const 4096))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32) (local $p i32)
      (local.set $p (i32.and (i32.add (global.get $heap) (i32.const 7)) (i32.const -8)))
      (global.set $heap (i32.add (local.get $p) (local.get 3)))
      (local.get $p)))
  (core instance $rm (instantiate $rmem))
  (alias export $exec "invoke" (func $invoke))
  (core func $invoke-lower (canon lower (func $invoke) (memory $rm "memory")
    (realloc (func $rm "realloc"))))
  (core module $rcode
    (import "m" "memory" (memory 1))
    (import "h" "invoke" (func $invoke (param i32 i32 i32 i32 i32)))
    (func (export "run") (param i32 i32) (result i32)
      (call $invoke (local.get 0) (local.get 1) (i32.const 0) (i32.const 0) (i32.const 2048))
      (i32.const 2048)))
  (core instance $rc (instantiate $rcode (with "m" (instance $rm))
    (with "h" (instance (export "invoke" (func $invoke-lower))))))
  (type $lerror (record (field "code" string) (field "message" string)
    (field "category" string) (field "severity" string) (field "retryable" bool)
    (field "retry-after-ms" (option u64)) (field "attributes" (option string))))
  (type $signal (record (field "checkpoint-id" string) (field "deadline-ms" (option u64))))
  (type $lwake (variant (case "at" u64) (case "on-signal" $signal) (case "on-resume")))
  (type $loutcome (variant (case "completed" (list u8)) (case "suspended" (list $lwake))))
  (func $run async (param "input" (list u8)) (result (result $loutcome (error $lerror)))
    (canon lift (core func $rc "run") (memory $rm "memory") (realloc (func $rm "realloc"))))
  (instance $lifecycle
    (export "error-info" (type $lerror)) (export "signal-wait" (type $signal))
    (export "wake" (type $lwake)) (export "outcome" (type $loutcome))
    (export "invoke" (func $run)))
  (export "runtara:workflow-lifecycle/lifecycle@0.2.0" (instance $lifecycle))
