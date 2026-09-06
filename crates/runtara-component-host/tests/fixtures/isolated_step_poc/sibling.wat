(component
  (import "mark-started" (func $mark))
  (import "receive" (func $receive (result u32)))
  (core func $mark (canon lower (func $mark)))
  (core func $receive (canon lower (func $receive)))
  (core module $code
    (import "host" "mark" (func $mark))
    (import "host" "receive" (func $receive (result i32)))
    (memory 1)
    (func (export "run") (param i32) (result i32)
      (i32.store (i32.const 0) (local.get 0))
      call $mark
      ;; The sibling remains live until the parent explicitly releases it.
      (if (i32.ne (call $receive) (i32.const 7)) (then unreachable))
      (if (i32.ne (i32.load (i32.const 0)) (local.get 0)) (then unreachable))
      i32.const 7))
  (core instance $host (export "mark" (func $mark)) (export "receive" (func $receive)))
  (core instance $code (instantiate $code (with "host" (instance $host))))
  (func (export "run") (param "input" u32) (result u32)
    (canon lift (core func $code "run"))))
