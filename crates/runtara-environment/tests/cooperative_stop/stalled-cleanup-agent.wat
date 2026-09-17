;; Adversarial Agent fixture, not production code. It uses the normal HTTP Agent
;; interface and standard callback ABI. Invoke starts a request that never gets
;; a response. Event 6 proves the parent requested standard subtask cancellation;
;; its callback then starts a cleanup request that also never gets a response.
;; It deliberately never calls task.cancel/task.return. The parent must remain
;; blocked in subtask.cancel until the host aborts the entire execution.
(component
  (import "runtara:host-io/http@0.1.0" (instance $http
    (export "request" (func async (param "input" (list u8))
      (result (result (list u8) (error string)))))))
  (core module $memory
    (memory (export "memory") 2)
    (global $heap (mut i32) (i32.const 8192))
    (func (export "realloc") (param i32 i32 i32) (param $size i32) (result i32) (local $p i32)
      (local.set $p (global.get $heap))
      (global.set $heap (i32.and (i32.add (i32.add (global.get $heap) (local.get $size)) (i32.const 15)) (i32.const -16)))
      (local.get $p)))
  (core instance $memory (instantiate $memory))
  (core func $request (canon lower (func $http "request") async
    (memory $memory "memory") (realloc (func $memory "realloc"))))
  (core func $new (canon waitable-set.new))
  (core func $join (canon waitable.join))
  (core module $code
    (import "m" "memory" (memory 2))
    (import "h" "request" (func $request (param i32 i32 i32) (result i32)))
    (import "h" "new" (func $new (result i32)))
    (import "h" "join" (func $join (param i32 i32)))
    (data (i32.const 1024) "{{REQUEST}}")
    (data (i32.const 4096) "{{CLEANUP}}")
    (global $set (mut i32) (i32.const 0))
    (global $cancelling (mut i32) (i32.const 0))
    (func $pending (param $status i32)
      ;; Neither endpoint responds. An eager completion is a fixture failure.
      (if (i32.ne (i32.and (local.get $status) (i32.const 15)) (i32.const 1)) (then unreachable))
      (call $join (i32.shr_u (local.get $status) (i32.const 4)) (global.get $set)))
    (func $wait (result i32)
      (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
    (func (export "invoke") (param i32 i32 i32 i32) (result i32)
      (global.set $set (call $new))
      (call $pending (call $request (i32.const 1024) (i32.const {{REQUEST_LEN}}) (i32.const 0)))
      (call $wait))
    (func (export "callback") (param $event i32) (param i32 i32) (result i32)
      (if (i32.ne (local.get $event) (i32.const 6)) (then unreachable))
      (if (global.get $cancelling) (then unreachable))
      (global.set $cancelling (i32.const 1))
      ;; The test observes this distinct request before expecting grace abort.
      ;; The original I/O remains live: cleanup has begun but has not finished.
      (call $pending (call $request (i32.const 4096) (i32.const {{CLEANUP_LEN}}) (i32.const 64)))
      (call $wait)))
  (core instance $code (instantiate $code
    (with "m" (instance $memory))
    (with "h" (instance (export "request" (func $request))
      (export "new" (func $new)) (export "join" (func $join))))))
  (type $error (record (field "code" string) (field "message" string)
    (field "category" string) (field "severity" string) (field "retryable" bool)
    (field "retry-after-ms" (option u64)) (field "attributes" (option string))))
  (func $invoke async (param "capability-id" string) (param "input" (list u8))
    (result (result (list u8) (error $error)))
    (canon lift (core func $code "invoke") async (callback (func $code "callback"))
      (memory $memory "memory") (realloc (func $memory "realloc"))))
  (instance $capabilities
    (export "error-info" (type $error))
    (export "invoke" (func $invoke)))
  (export "runtara:agent-http/capabilities@0.4.0" (instance $capabilities)))
