;; Fixtures for control_tests.rs. Sections are split on the `;;--` markers.
;;-- TYPES
  (type $error (record (field "code" string) (field "message" string)
    (field "category" string) (field "severity" string) (field "retryable" bool)
    (field "retry-after-ms" (option u64)) (field "attributes" (option string)) (field "details" (option string))))
;;-- EXECUTOR
  (import "runtara:control/executor@1.0.0" (instance $exec
    (type $error-def (record (field "code" string) (field "message" string)
      (field "category" string) (field "severity" string) (field "retryable" bool)
      (field "retry-after-ms" (option u64)) (field "attributes" (option string)) (field "details" (option string))))
    (export "error-info" (type $e (eq $error-def)))
    (export "invoke" (func async (param "capability-id" string) (param "input" (list u8))
      (result (result (list u8) (error $e)))))))
;;-- API
  (import "runtara:control/api@1.0.0" (instance $api
    (type $outcome-def (enum "requested" "applied" "unchanged" "already-terminal"))
    (export "command-outcome" (type $outcome (eq $outcome-def)))
    (type $result-def (record (field "instance-id" string) (field "outcome" $outcome)
      (field "replayed" bool)))
    (export "command-result" (type $result (eq $result-def)))
    (type $code-def (enum "denied" "invalid" "not-found" "not-runnable" "not-child"
      "requires-instance" "requires-operation" "capacity" "replay-conflict" "label-conflict"
      "too-large" "unavailable" "unsupported" "not-waiting" "ambiguous" "already-answered"
      "not-pausable" "not-paused"))
    (export "error-code" (type $code (eq $code-def)))
    (type $cerror-def (record (field "code" $code) (field "message" string)
      (field "retry-after-ms" (option u64))))
    (export "control-error" (type $control-error (eq $cerror-def)))
    (export "pause" (func async (param "instance-id" string)
      (result (result $result (error $control-error)))))))
;;-- AGENT
  ;; The control agent fixture: imports the executor and the API, exports
  ;; `execution`. By the capability id's first byte: `p` calls the API's
  ;; `pause` and answers "paused", `b` answers 4 MiB + 1 bytes, anything else
  ;; answers "ok".
  {{EXECUTOR}}
  {{API}}
  (alias export $api "pause" (func $pause))
  (core module $memory
    (memory (export "memory") 70)
    (global $heap (mut i32) (i32.const 8192))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32) (local $p i32)
      (local.set $p (i32.and (i32.add (global.get $heap) (i32.const 7)) (i32.const -8)))
      (global.set $heap (i32.add (local.get $p) (local.get 3)))
      (local.get $p)))
  (core instance $memory (instantiate $memory))
  (core func $pause-lower (canon lower (func $pause) (memory $memory "memory")
    (realloc (func $memory "realloc"))))
  (core module $code
    (import "m" "memory" (memory 70))
    (import "h" "pause" (func $pause (param i32 i32 i32)))
    (data (i32.const 1024) "w")
    (data (i32.const 1040) "\22paused\22")
    (data (i32.const 1056) "\22ok\22")
    (func (export "execute") (param i32 i32 i32 i32) (result i32)
      (local $c i32)
      (local.set $c (i32.load8_u (local.get 0)))
      (i32.store8 (i32.const 2048) (i32.const 0))
      (if (i32.eq (local.get $c) (i32.const 112))
        (then
          (call $pause (i32.const 1024) (i32.const 1) (i32.const 3072))
          (i32.store (i32.const 2056) (i32.const 1040))
          (i32.store (i32.const 2060) (i32.const 8))
          (return (i32.const 2048))))
      (if (i32.eq (local.get $c) (i32.const 98))
        (then
          (i32.store (i32.const 2056) (i32.const 0))
          (i32.store (i32.const 2060) (i32.const 4194305))
          (return (i32.const 2048))))
      (i32.store (i32.const 2056) (i32.const 1056))
      (i32.store (i32.const 2060) (i32.const 4))
      (i32.const 2048)))
  (core instance $code (instantiate $code
    (with "m" (instance $memory))
    (with "h" (instance (export "pause" (func $pause-lower))))))
  {{TYPES}}
  (func $execute async (param "capability-id" string) (param "input" (list u8))
    (result (result (list u8) (error $error)))
    (canon lift (core func $code "execute") (memory $memory "memory")
      (realloc (func $memory "realloc"))))
  (instance $execution (export "error-info" (type $error))
    (export "invoke" (func $execute)))
  (export "runtara:control/execution@1.0.0" (instance $execution))
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
    ;; An ok output (list at +8) becomes `completed` (list at +12); an
    ;; error-info is laid out the same in both results.
    (func (export "run") (param i32 i32 i32 i32) (result i32)
      (call $invoke (local.get 2) (local.get 3) (i32.const 0) (i32.const 0) (i32.const 2048))
      (if (i32.eqz (i32.load8_u (i32.const 2048)))
        (then
          (i32.store (i32.const 2064) (i32.load (i32.const 2060)))
          (i32.store (i32.const 2060) (i32.load (i32.const 2056)))
          (i32.store8 (i32.const 2056) (i32.const 0))))
      (i32.const 2048)))
  (core instance $rc (instantiate $rcode (with "m" (instance $rm))
    (with "h" (instance (export "invoke" (func $invoke-lower))))))
  (type $lerror (record (field "code" string) (field "message" string)
    (field "category" string) (field "severity" string) (field "retryable" bool)
    (field "retry-after-ms" (option u64)) (field "attributes" (option string)) (field "details" (option string))))
  (type $signal (record (field "checkpoint-id" string) (field "deadline-ms" (option u64))))
  (type $lwake (variant (case "at" u64) (case "on-signal" $signal) (case "on-resume")
    (case "instances" string)))
  (type $lsuspension (record (field "wakes" (list $lwake)) (field "state" (list u8))))
  (type $loutcome (variant (case "completed" (list u8)) (case "suspended" $lsuspension)))
  (func $run async (param "capability-id" string) (param "input" (list u8)) (result (result $loutcome (error $lerror)))
    (canon lift (core func $rc "run") (memory $rm "memory") (realloc (func $rm "realloc"))))
  (instance $lifecycle
    (export "error-info" (type $lerror)) (export "signal-wait" (type $signal))
    (export "wake" (type $lwake)) (export "suspension" (type $lsuspension))
    (export "outcome" (type $loutcome))
    (export "invoke" (func $run)))
  (export "runtara:agent-workflow-agent/capabilities@1.0.0" (instance $lifecycle))
