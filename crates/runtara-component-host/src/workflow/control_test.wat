;; Fixtures for control_tests.rs. Sections are split on the `;;--` markers.
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
      "not-pausable" "not-paused" "timeout"))
    (export "error-code" (type $code (eq $code-def)))
    (type $cerror-def (record (field "code" $code) (field "message" string)
      (field "retry-after-ms" (option u64))))
    (export "control-error" (type $control-error (eq $cerror-def)))
    (export "pause" (func async (param "instance-id" string)
      (result (result $result (error $control-error)))))))
;;-- DIRECT
  ;; A workflow root whose own code calls the API's `pause` with its input as
  ;; the instance id. It completes with "paused", or with `"X"` where X is `A`
  ;; plus the error code's index (`A` denied, `S` timeout).
  {{API}}
  (alias export $api "pause" (func $pause))
  (core module $rmem
    (memory (export "memory") 1)
    (global $heap (mut i32) (i32.const 4096))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32) (local $p i32)
      (local.set $p (i32.and (i32.add (global.get $heap) (i32.const 7)) (i32.const -8)))
      (global.set $heap (i32.add (local.get $p) (local.get 3)))
      (local.get $p)))
  (core instance $rm (instantiate $rmem))
  (core func $pause-lower (canon lower (func $pause) (memory $rm "memory")
    (realloc (func $rm "realloc"))))
  (core module $rcode
    (import "m" "memory" (memory 1))
    (import "h" "pause" (func $pause (param i32 i32 i32)))
    (data (i32.const 1040) "\22paused\22")
    ;; `completed` (outcome case 0) with its list at +12.
    (func (export "run") (param i32 i32 i32 i32) (result i32)
      (call $pause (local.get 2) (local.get 3) (i32.const 3072))
      (i32.store8 (i32.const 2048) (i32.const 0))
      (i32.store8 (i32.const 2056) (i32.const 0))
      (if (i32.eqz (i32.load8_u (i32.const 3072)))
        (then
          (i32.store (i32.const 2060) (i32.const 1040))
          (i32.store (i32.const 2064) (i32.const 8))
          (return (i32.const 2048))))
      ;; control-error sits at +8; its code is the first byte.
      (i32.store8 (i32.const 1100) (i32.const 34))
      (i32.store8 (i32.const 1101) (i32.add (i32.const 65) (i32.load8_u (i32.const 3080))))
      (i32.store8 (i32.const 1102) (i32.const 34))
      (i32.store (i32.const 2060) (i32.const 1100))
      (i32.store (i32.const 2064) (i32.const 3))
      (i32.const 2048)))
  (core instance $rc (instantiate $rcode (with "m" (instance $rm))
    (with "h" (instance (export "pause" (func $pause-lower))))))
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
