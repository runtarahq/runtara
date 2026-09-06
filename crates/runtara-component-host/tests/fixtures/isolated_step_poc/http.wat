;; Uses the production host-io HTTP binding. The server accepts the connection
;; and deliberately never finishes its response; the request budget is 120s.
(component
  (import "runtara:host-io/http@0.1.0" (instance $http
    (export "request" (func async (param "input" (list u8)) (result (result (list u8) (error string)))))))
  (alias export $http "request" (func $request))
  (core module $mem
    (memory (export "memory") 4)
    (global $next (mut i32) (i32.const 8192))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32)
      (local $ptr i32)
      (local.set $ptr (global.get $next))
      (global.set $next (i32.and (i32.add (i32.add (global.get $next) (local.get 3)) (i32.const 15)) (i32.const -16)))
      local.get $ptr))
  (core instance $mem (instantiate $mem))
  (core func $request (canon lower (func $request) (memory $mem "memory") (realloc (func $mem "realloc"))))
  (core module $code
    (import "mem" "memory" (memory 4))
    (import "host" "request" (func $request (param i32 i32 i32)))
    (data (i32.const 1024) "{{REQUEST}}")
    (func (export "run") (param i32) (result i32)
      (call $request (i32.const 1024) (i32.const {{REQUEST_LEN}}) (i32.const 64))
      ;; Any normal completion is a failed cancellation proof.
      i32.const 99))
  (core instance $host (export "request" (func $request)))
  (core instance $code (instantiate $code (with "host" (instance $host)) (with "mem" (instance $mem))))
  (func (export "run") async (param "input" u32) (result u32)
    (canon lift (core func $code "run"))))
