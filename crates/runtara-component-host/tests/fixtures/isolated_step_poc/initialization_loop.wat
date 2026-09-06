(component
  (import "mark-started" (func $mark))
  (core func $mark (canon lower (func $mark)))
  (core module $code
    (import "host" "mark" (func $mark))
    (func $initialize
      call $mark
      (loop $forever br $forever))
    (start $initialize)
    (func (export "run") (param i32) (result i32) i32.const 99))
  (core instance $host (export "mark" (func $mark)))
  (core instance $code (instantiate $code (with "host" (instance $host))))
  (func (export "run") (param "input" u32) (result u32)
    (canon lift (core func $code "run"))))
