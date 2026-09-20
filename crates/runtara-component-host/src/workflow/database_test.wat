(component
  (import "runtara:database/sql@0.1.0" (instance $database
    (export "query" (func async (param "connection-id" string) (param "request" (list u8)) (result (result (list u8) (error string)))))
    (export "execute" (func async (param "connection-id" string) (param "request" (list u8)) (result (result (list u8) (error string)))))
    (export "execute-batch" (func async (param "connection-id" string) (param "request" (list u8)) (result (result (list u8) (error string)))))))
  (core module $memory
    (memory (export "memory") 1)
    (global $heap (mut i32) (i32.const 4096))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32)
      (local $ptr i32)
      global.get $heap i32.const 7 i32.add i32.const -8 i32.and local.tee $ptr
      local.get 3 i32.add global.set $heap local.get $ptr))
  (core instance $mem (instantiate $memory))
  (alias export $database "query" (func $query))
  (alias export $database "execute" (func $execute))
  (alias export $database "execute-batch" (func $batch))
  (core func $query (canon lower (func $query) (memory $mem "memory") (realloc (func $mem "realloc"))))
  (core func $execute (canon lower (func $execute) (memory $mem "memory") (realloc (func $mem "realloc"))))
  (core func $batch (canon lower (func $batch) (memory $mem "memory") (realloc (func $mem "realloc"))))
  (core module $code
    (import "mem" "memory" (memory 1))
    (import "host" "query" (func $query (param i32 i32 i32 i32 i32)))
    (import "host" "execute" (func $execute (param i32 i32 i32 i32 i32)))
    (import "host" "batch" (func $batch (param i32 i32 i32 i32 i32)))
    (data (i32.const 0) "conn{\22sql\22:\22SELECT 1\22}")
    (data (i32.const 64) "{\22statements\22:[]}")
    (func $check
      i32.const 256 i32.load if unreachable end
      i32.const 264 i32.load i32.eqz if unreachable end)
    (func (export "invoke") (param i32 i32) (result i32)
      (call $query (i32.const 0) (i32.const 4) (i32.const 4) (i32.const 18) (i32.const 256)) call $check
      (call $execute (i32.const 0) (i32.const 4) (i32.const 4) (i32.const 18) (i32.const 256)) call $check
      (call $batch (i32.const 0) (i32.const 4) (i32.const 64) (i32.const 17) (i32.const 256)) call $check
      (i32.store (i32.const 2048) (i32.const 0))
      (i32.store (i32.const 2056) (i32.const 0))
      (i32.store (i32.const 2060) (i32.load (i32.const 260)))
      (i32.store (i32.const 2064) (i32.load (i32.const 264)))
      i32.const 2048))
  (core instance $code (instantiate $code (with "mem" (instance $mem))
    (with "host" (instance (export "query" (func $query)) (export "execute" (func $execute)) (export "batch" (func $batch))))))
  (type $error (record (field "code" string) (field "message" string) (field "category" string)
    (field "severity" string) (field "retryable" bool) (field "retry-after-ms" (option u64)) (field "attributes" (option string))))
  (type $signal (record (field "checkpoint-id" string) (field "deadline-ms" (option u64))))
  (type $wake (variant (case "at" u64) (case "on-signal" $signal) (case "on-resume")))
  (type $outcome (variant (case "completed" (list u8)) (case "suspended" (list $wake))))
  (func $invoke async (param "input" (list u8)) (result (result $outcome (error $error)))
    (canon lift (core func $code "invoke") (memory $mem "memory") (realloc (func $mem "realloc"))))
  (instance $lifecycle
    (export "error-info" (type $error)) (export "signal-wait" (type $signal)) (export "wake" (type $wake))
    (export "outcome" (type $outcome)) (export "invoke" (func $invoke)))
  (export "runtara:workflow-lifecycle/lifecycle@0.2.0" (instance $lifecycle)))
