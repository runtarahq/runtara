;; Adversarial Agent fixture, not production code. It uses the normal HTTP Agent
;; interface and standard callback ABI. Invoke starts a request that never gets
;; a response. Event 6 proves the parent requested standard subtask cancellation;
;; its callback then starts a cleanup request that also never gets a response.
;; It deliberately never calls task.cancel/task.return. The parent must remain
;; blocked in subtask.cancel until the host aborts the entire execution.
(component
  (import "runtara:outbound-http/client@0.1.0" (instance $http
                (type $connection-def (record (field "connection-id" string) (field "url" string)
                    (field "endpoint" (option string)) (field "endpoint-ref" (option string))
                    (field "ai-provider" (option string)) (field "aws-service" (option string))))
                (export "connection-destination" (type $connection (eq $connection-def)))
                (type $destination-def (variant (case "connection" $connection) (case "public" string)))
                (export "destination" (type $destination (eq $destination-def)))
                (type $headers (list (tuple string string)))
                (type $request-def (record (field "destination" $destination) (field "method" string)
                    (field "headers" $headers) (field "body" (option (list u8)))
                    (field "timeout-ms" (option u64)) (field "max-response-bytes" (option u64))))
                (export "request-options" (type $request (eq $request-def)))
                (type $response-def (record (field "status" u16) (field "headers" $headers) (field "body" (list u8))))
                (export "response" (type $response (eq $response-def)))
                (type $error-def (record (field "code" string) (field "message" string)
                    (field "status" (option u16)) (field "body" (list u8)) (field "retry-after-ms" (option u64))))
                (export "outbound-error" (type $error (eq $error-def)))
                (export "request" (func async (param "options" $request) (result (result $response (error $error)))))))
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
    (import "h" "request" (func $request (param i32 i32) (result i32)))
    (import "h" "new" (func $new (result i32)))
    (import "h" "join" (func $join (param i32 i32)))
    (data (i32.const 1024) "{{REQUEST}}")
    (data (i32.const 4096) "{{CLEANUP}}")
    (data (i32.const 768) "GET")
    (global $set (mut i32) (i32.const 0))
    (global $cancelling (mut i32) (i32.const 0))
    ;; Canonical request-options layout: destination (68 bytes), method,
    ;; headers, optional body, optional timeout, optional response limit.
    ;; Memory starts zeroed, so absent headers/body/limit need no stores.
    (func $options (param $p i32) (param $url i32) (param $len i32)
      (i32.store8 (local.get $p) (i32.const 1)) ;; public destination
      (i32.store offset=4 (local.get $p) (local.get $url))
      (i32.store offset=8 (local.get $p) (local.get $len))
      (i32.store offset=68 (local.get $p) (i32.const 768))
      (i32.store offset=72 (local.get $p) (i32.const 3))
      (i32.store8 offset=96 (local.get $p) (i32.const 1))
      (i64.store offset=104 (local.get $p) (i64.const 120000)))
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
      (call $pending (call $request (i32.const 512) (i32.const 64)))
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
