;; Synthetic Agent ABI fixture: an ordinary agent whose only capability fails
;; with what used to be a reserved park code. It imports nothing, so the agent
;; import allowlist admits it from any components dir. Tests substitute
;; `__rt_suspended__` for `__rt_on_signal__` with a plain string replace; either
;; must now reach the parent as an ordinary error, never a park.
(component
  (type $error (record (field "code" string) (field "message" string)
    (field "category" string) (field "severity" string) (field "retryable" bool)
    (field "retry-after-ms" (option u64)) (field "attributes" (option string))))
  (type $signal (record (field "checkpoint-id" string) (field "deadline-ms" (option u64))))
  (type $wake (variant (case "at" u64) (case "on-signal" $signal) (case "on-resume")
    (case "instances" string)))
  (type $suspension (record (field "wakes" (list $wake)) (field "state" (list u8))))
  (type $outcome (variant (case "completed" (list u8)) (case "suspended" $suspension)))
  (core module $memory
    (memory (export "memory") 1)
    (global $heap (mut i32) (i32.const 4096))
    (func (export "realloc") (param i32 i32 i32) (param $size i32) (result i32) (local $p i32)
      (local.set $p (global.get $heap))
      (global.set $heap (i32.add (global.get $heap) (local.get $size)))
      (local.get $p)))
  (core instance $memory (instantiate $memory))
  (core func $return (canon task.return (result (result $outcome (error $error))) (memory $memory "memory")))
  (core module $code
    (import "m" "memory" (memory 1))
    (import "h" "return" (func $return (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i64 i32 i32 i32)))
    (data (i32.const 1024) "__rt_on_signal__")
    (data (i32.const 1056) "approval")
    (data (i32.const 1088) "permanent")
    (data (i32.const 1120) "error")
    (func (export "invoke") (param i32 i32 i32 i32) (result i32)
      ;; err(error-info { code, message: "approval", category: "permanent",
      ;; severity: "error", retryable: false, retry-after-ms: none,
      ;; attributes: none }), flattened to the 15 canonical values.
      (call $return (i32.const 1)
        (i32.const 1024) (i32.const 16)
        (i32.const 1056) (i32.const 8)
        (i32.const 1088) (i32.const 9)
        (i32.const 1120) (i32.const 5)
        (i32.const 0)
        (i32.const 0) (i64.const 0)
        (i32.const 0) (i32.const 0) (i32.const 0))
      (i32.const 0))
    (func (export "callback") (param i32 i32 i32) (result i32)
      unreachable))
  (core instance $code (instantiate $code
    (with "m" (instance $memory))
    (with "h" (instance (export "return" (func $return))))))
  (func $invoke async (param "capability-id" string) (param "input" (list u8))
    (result (result $outcome (error $error)))
    (canon lift (core func $code "invoke") async (callback (func $code "callback"))
      (memory $memory "memory") (realloc (func $memory "realloc"))))
  (instance $capabilities
    (export "error-info" (type $error))
    (export "signal-wait" (type $signal))
    (export "wake" (type $wake))
    (export "suspension" (type $suspension))
    (export "outcome" (type $outcome))
    (export "invoke" (func $invoke)))
  (export "runtara:agent-reserved-code/capabilities@1.0.0" (instance $capabilities)))
