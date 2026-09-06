(component
  (import "mark-started" (func $mark))
  (core func $mark (canon lower (func $mark)))
  (core module $code
    (import "host" "mark" (func $mark))
    (memory 1)
    (func (export "run") (param i32) (result i32)
      call $mark
      ;; Arbitrary, non-cooperative guest code: no await, poll, or cancel check.
      (loop $forever
        (i32.store (i32.const 0) (i32.add (i32.load (i32.const 0)) (i32.const 1)))
        br $forever)
      unreachable))
  (core instance $host (export "mark" (func $mark)))
  (core instance $code (instantiate $code (with "host" (instance $host))))
  (func (export "run") (param "input" u32) (result u32)
    (canon lift (core func $code "run"))))
