;; Child component binaries are embedded in this artifact's data segments.
;; Rust substitutes only bytes/lengths. All scheduling/error policy is here.
(component
  (import "spawn" (func $spawn (param "component" (list u8)) (param "input" u32) (result u32)))
  (import "cancel" (func $cancel (param "task" u32)))
  (import "join" (func $join (param "task" u32) (result u64)))
  (import "send" (func $send (param "task" u32) (param "message" u32)))
  (import "receive-command" (func $command (result u32)))
  (core module $mem (memory (export "memory") 16))
  (core instance $mem (instantiate $mem))
  (core func $spawn (canon lower (func $spawn) (memory $mem "memory")))
  (core func $cancel (canon lower (func $cancel)))
  (core func $join (canon lower (func $join)))
  (core func $send (canon lower (func $send)))
  (core func $command (canon lower (func $command)))
  (core module $code
    (import "mem" "memory" (memory 16))
    (import "host" "spawn" (func $spawn (param i32 i32 i32) (result i32)))
    (import "host" "cancel" (func $cancel (param i32)))
    (import "host" "join" (func $join (param i32) (result i64)))
    (import "host" "send" (func $send (param i32 i32)))
    (import "host" "command" (func $command (result i32)))
    (data (i32.const 4096) "{{VICTIM}}")
    (data (i32.const 131072) "{{SIBLING}}")
    (data (i32.const 262144) "{{RECOVERY}}")
    (func (export "run") (result i32)
      (local $victim i32) (local $sibling i32) (local $recovery i32)
      (i32.store (i32.const 0) (i32.const 123456))
      (local.set $victim (call $spawn (i32.const 4096) (i32.const {{VICTIM_LEN}}) (i32.const 17)))
      (local.set $sibling (call $spawn (i32.const 131072) (i32.const {{SIBLING_LEN}}) (i32.const 42)))
      ;; An external user event identifies a task; the GUEST decides to cancel.
      (if (i32.ne (call $command) (local.get $victim)) (then unreachable))
      {{BEFORE_CANCEL}}
      (call $cancel (local.get $victim))
      ;; join returns only after the victim Store has been destroyed.
      (if (i64.ne (call $join (local.get $victim)) (i64.const 4294967296)) (then unreachable))
      ;; Recovery/onError analogue lives in the guest, not the executor.
      (local.set $recovery (call $spawn (i32.const 262144) (i32.const {{RECOVERY_LEN}}) (i32.const 11)))
      (if (i64.ne (call $join (local.get $recovery)) (i64.const 11)) (then unreachable))
      (call $send (local.get $sibling) (i32.const 7))
      (if (i64.ne (call $join (local.get $sibling)) (i64.const 7)) (then unreachable))
      (if (i32.ne (i32.load (i32.const 0)) (i32.const 123456)) (then unreachable))
      i32.const 42))
  (core instance $host
    (export "spawn" (func $spawn)) (export "cancel" (func $cancel))
    (export "join" (func $join)) (export "send" (func $send))
    (export "command" (func $command)))
  (core instance $code (instantiate $code (with "host" (instance $host)) (with "mem" (instance $mem))))
  (func (export "run") (result u32) (canon lift (core func $code "run"))))
