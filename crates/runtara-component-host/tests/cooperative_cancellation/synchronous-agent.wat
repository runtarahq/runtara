;; The existing Rust agents' ABI shape: async-typed export, synchronous lift,
;; synchronous lowering of the async-typed I/O import. No cancellation callback.
(component $agent
  (import "request" (func $request async (result u32)))
  (import "trace" (func $trace (param "event" u32)))
  (core func $request (canon lower (func $request)))
  (core module $code
    (import "h" "request" (func $request (result i32)))
    (global $calls (mut i32) (i32.const 0))
    (func (export "run") (result i32)
      (global.set $calls (i32.add (global.get $calls) (i32.const 1)))
      (i32.add (call $request) (global.get $calls))))
  (core instance $code (instantiate $code
    (with "h" (instance (export "request" (func $request))))))
  (func (export "run") async (result u32) (canon lift (core func $code "run"))))
