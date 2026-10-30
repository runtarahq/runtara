;; Adversarial Agent fixture, not production code. It calls the workflow API's
;; `runtara:api/connections.http` (the test composes it with the api component)
;; and uses the standard callback ABI. Invoke starts a request that never gets
;; a response. Event 6 proves the parent requested standard subtask cancellation;
;; its callback then starts a cleanup request that also never gets a response.
;; It deliberately never calls task.cancel/task.return. The parent must remain
;; blocked in subtask.cancel until the host aborts the entire execution.
(component
  (import "runtara:api/types@1.0.0" (instance $types
    (type $error-def (record (field "code" string) (field "message" string)
      (field "retry-after-ms" (option u64)) (field "details" (option string))))
    (export "api-error" (type (eq $error-def)))))
  (alias export $types "api-error" (type $api-error))
  (import "runtara:api/connections@1.0.0" (instance $http
    (alias outer 1 $api-error (type $api-error-outer))
    (export "api-error" (type $api-error (eq $api-error-outer)))
    (type $target-def (record (field "connection-id" string) (field "url" string)
      (field "endpoint" (option string)) (field "endpoint-ref" (option string))
      (field "ai-provider" (option string)) (field "aws-service" (option string))))
    (export "connection-target" (type $target (eq $target-def)))
    (type $destination-def (variant (case "connection" $target) (case "public" string)))
    (export "destination" (type $destination (eq $destination-def)))
    (type $headers (list (tuple string string)))
    (type $request-def (record (field "method" string) (field "destination" $destination)
      (field "headers" $headers) (field "timeout-ms" (option u64)) (field "max-response-bytes" (option u64))))
    (export "outbound-request" (type $request (eq $request-def)))
    (type $reply-def (record (field "status" u16) (field "headers" $headers) (field "body" (list u8))))
    (export "outbound-reply" (type $reply (eq $reply-def)))
    (type $failure-def (record (field "code" string) (field "message" string)
      (field "status" (option u16)) (field "retry-after-ms" (option u64)) (field "body" (list u8))))
    (export "outbound-failure" (type $failure (eq $failure-def)))
    (export "http" (func async (param "request" $request) (param "body" (option (list u8)))
      (result (result (result $reply (error $failure)) (error $api-error)))))))
  (core module $memory
    (memory (export "memory") 2)
    (global $heap (mut i32) (i32.const 8192))
    (func (export "realloc") (param i32 i32 i32) (param $size i32) (result i32) (local $p i32)
      (local.set $p (global.get $heap))
      (global.set $heap (i32.and (i32.add (i32.add (global.get $heap) (local.get $size)) (i32.const 15)) (i32.const -16)))
      (local.get $p)))
  (core instance $memory (instantiate $memory))
  (core func $request (canon lower (func $http "http") async
    (memory (core memory $memory "memory")) (realloc (core func $memory "realloc"))))
  (core func $new (canon waitable-set.new))
  (core func $join (canon waitable.join))
  (core module $code
    (import "m" "memory" (memory 2))
    (import "h" "request" (func $request (param i32 i32) (result i32)))
    (import "h" "new" (func $new (result i32)))
    (import "h" "join" (func $join (param i32 i32)))
    (data (i32.const 1024) "{{REQUEST}}")
    (data (i32.const 4096) "{{CLEANUP}}")
    (data (i32.const 768) "GET")
    (global $set (mut i32) (i32.const 0))
    (global $cancelling (mut i32) (i32.const 0))
    ;; Canonical (outbound-request, option<list<u8>>) layout: method, the
    ;; destination variant (68 bytes from 8), headers, optional timeout,
    ;; optional response limit, then the absent body. Memory starts zeroed,
    ;; so absent headers/limit/body need no stores.
    (func $options (param $p i32) (param $url i32) (param $len i32)
      (i32.store (local.get $p) (i32.const 768))
      (i32.store offset=4 (local.get $p) (i32.const 3))
      (i32.store8 offset=8 (local.get $p) (i32.const 1)) ;; public destination
      (i32.store offset=12 (local.get $p) (local.get $url))
      (i32.store offset=16 (local.get $p) (local.get $len))
      (i32.store8 offset=88 (local.get $p) (i32.const 1))
      (i64.store offset=96 (local.get $p) (i64.const 120000)))
    (func $pending (param $status i32)
      ;; Neither endpoint responds. An eager completion is a fixture failure.
      (if (i32.ne (i32.and (local.get $status) (i32.const 15)) (i32.const 1)) (then unreachable))
      (call $join (i32.shr_u (local.get $status) (i32.const 4)) (global.get $set)))
    (func $wait (result i32)
      (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
    (func (export "invoke") (param i32 i32 i32 i32) (result i32)
      (global.set $set (call $new))
      (call $options (i32.const 256) (i32.const 1024) (i32.const {{REQUEST_LEN}}))
      (call $pending (call $request (i32.const 256) (i32.const 0)))
      (call $wait))
    (func (export "callback") (param $event i32) (param i32 i32) (result i32)
      (if (i32.ne (local.get $event) (i32.const 6)) (then unreachable))
      (if (global.get $cancelling) (then unreachable))
      (global.set $cancelling (i32.const 1))
      ;; The test observes this distinct request before expecting grace abort.
      ;; The original I/O remains live: cleanup has begun but has not finished.
      (call $options (i32.const 512) (i32.const 4096) (i32.const {{CLEANUP_LEN}}))
      (call $pending (call $request (i32.const 512) (i32.const 128)))
      (call $wait)))
  (core instance $code (instantiate $code
    (with "m" (instance $memory))
    (with "h" (instance (export "request" (func $request))
      (export "new" (func $new)) (export "join" (func $join))))))
  (type $error (record (field "code" string) (field "message" string)
    (field "category" string) (field "severity" string) (field "retryable" bool)
    (field "retry-after-ms" (option u64)) (field "attributes" (option string)) (field "details" (option string))))
  (type $signal (record (field "checkpoint-id" string) (field "deadline-ms" (option u64))))
  (type $wake (variant (case "at" u64) (case "on-signal" $signal) (case "on-resume")
    (case "instances" string)))
  (type $suspension (record (field "wakes" (list $wake)) (field "state" (list u8))))
  (type $outcome (variant (case "completed" (list u8)) (case "suspended" $suspension)))
  (func $invoke async (param "capability-id" string) (param "input" (list u8))
    (result (result $outcome (error $error)))
    (canon lift (core func $code "invoke") async (callback (core func $code "callback"))
      (memory (core memory $memory "memory")) (realloc (core func $memory "realloc"))))
  (instance $capabilities
    (export "error-info" (type $error))
    (export "signal-wait" (type $signal)) (export "wake" (type $wake))
    (export "suspension" (type $suspension)) (export "outcome" (type $outcome))
    (export "invoke" (func $invoke)))
  (export "runtara:agent-http/capabilities@1.0.0" (instance $capabilities)))
