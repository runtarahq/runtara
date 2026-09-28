;; Frozen ABI fixture: a guest compiled against the released 1.0.0 WIT
;; (wit-component dummy guest, printed by wasm-tools 1.249.0). Never edit or
;; regenerate this file: it must keep linking for as long as the host runs
;; artifacts built against 1.0.0. See runtara-wit's versioning rule.
(component
  (type $ty-runtara:workflow/waits@1.0.0 (;0;)
    (instance
      (type (;0;) (list u8))
      (type (;1;) (result 0 (error string)))
      (type (;2;) (func (param "key" string) (param "request" 0) (result 1)))
      (export (;0;) "register" (func (type 2)))
      (type (;3;) (func (param "key" string) (result 1)))
      (export (;1;) "poll" (func (type 3)))
      (type (;4;) (func (param "key" string)))
      (export (;2;) "release" (func (type 4)))
    )
  )
  (import "runtara:workflow/waits@1.0.0" (instance $runtara:workflow/waits@1.0.0 (;0;) (type $ty-runtara:workflow/waits@1.0.0)))
  (core module $main (;0;)
    (type (;0;) (func (param i32 i32 i32 i32 i32)))
    (type (;1;) (func (param i32 i32 i32)))
    (type (;2;) (func (param i32 i32)))
    (type (;3;) (func (param i32 i32 i32 i32) (result i32)))
    (type (;4;) (func))
    (import "cm32p2|runtara:workflow/waits@1" "register" (func (;0;) (type 0)))
    (import "cm32p2|runtara:workflow/waits@1" "poll" (func (;1;) (type 1)))
    (import "cm32p2|runtara:workflow/waits@1" "release" (func (;2;) (type 2)))
    (memory (;0;) 0)
    (export "cm32p2_memory" (memory 0))
    (export "cm32p2_realloc" (func 3))
    (export "cm32p2_initialize" (func 4))
    (func (;3;) (type 3) (param i32 i32 i32 i32) (result i32)
      unreachable
    )
    (func (;4;) (type 4))
    (@producers
      (processed-by "wit-component" "0.249.0")
    )
  )
  (core module $wit-component-shim-module (;1;)
    (type (;0;) (func (param i32 i32 i32 i32 i32)))
    (type (;1;) (func (param i32 i32 i32)))
    (type (;2;) (func (param i32 i32)))
    (table (;0;) 3 3 funcref)
    (export "0" (func 0))
    (export "1" (func 1))
    (export "2" (func 2))
    (export "$imports" (table 0))
    (func (;0;) (type 0) (param i32 i32 i32 i32 i32)
      local.get 0
      local.get 1
      local.get 2
      local.get 3
      local.get 4
      i32.const 0
      call_indirect (type 0)
    )
    (func (;1;) (type 1) (param i32 i32 i32)
      local.get 0
      local.get 1
      local.get 2
      i32.const 1
      call_indirect (type 1)
    )
    (func (;2;) (type 2) (param i32 i32)
      local.get 0
      local.get 1
      i32.const 2
      call_indirect (type 2)
    )
    (@producers
      (processed-by "wit-component" "0.249.0")
    )
  )
  (core module $wit-component-fixup (;2;)
    (type (;0;) (func (param i32 i32 i32 i32 i32)))
    (type (;1;) (func (param i32 i32 i32)))
    (type (;2;) (func (param i32 i32)))
    (import "" "0" (func (;0;) (type 0)))
    (import "" "1" (func (;1;) (type 1)))
    (import "" "2" (func (;2;) (type 2)))
    (import "" "$imports" (table (;0;) 3 3 funcref))
    (elem (;0;) (i32.const 0) func 0 1 2)
    (@producers
      (processed-by "wit-component" "0.249.0")
    )
  )
  (core instance $wit-component-shim-instance (;0;) (instantiate $wit-component-shim-module))
  (alias core export $wit-component-shim-instance "0" (core func $indirect-cm32p2|runtara:workflow/waits@1-register (;0;)))
  (alias core export $wit-component-shim-instance "1" (core func $indirect-cm32p2|runtara:workflow/waits@1-poll (;1;)))
  (alias core export $wit-component-shim-instance "2" (core func $indirect-cm32p2|runtara:workflow/waits@1-release (;2;)))
  (core instance $cm32p2|runtara:workflow/waits@1 (;1;)
    (export "register" (func $indirect-cm32p2|runtara:workflow/waits@1-register))
    (export "poll" (func $indirect-cm32p2|runtara:workflow/waits@1-poll))
    (export "release" (func $indirect-cm32p2|runtara:workflow/waits@1-release))
  )
  (core instance $main (;2;) (instantiate $main
      (with "cm32p2|runtara:workflow/waits@1" (instance $cm32p2|runtara:workflow/waits@1))
    )
  )
  (alias core export $main "cm32p2_memory" (core memory $memory (;0;)))
  (alias core export $wit-component-shim-instance "$imports" (core table $"shim table" (;0;)))
  (alias export $runtara:workflow/waits@1.0.0 "register" (func $register (;0;)))
  (alias core export $main "cm32p2_realloc" (core func $realloc (;3;)))
  (core func $"#core-func4 indirect-cm32p2|runtara:workflow/waits@1-register" (@name "indirect-cm32p2|runtara:workflow/waits@1-register") (;4;) (canon lower (func $register) (memory $memory) (realloc $realloc) string-encoding=utf8))
  (alias export $runtara:workflow/waits@1.0.0 "poll" (func $poll (;1;)))
  (core func $"#core-func5 indirect-cm32p2|runtara:workflow/waits@1-poll" (@name "indirect-cm32p2|runtara:workflow/waits@1-poll") (;5;) (canon lower (func $poll) (memory $memory) (realloc $realloc) string-encoding=utf8))
  (alias export $runtara:workflow/waits@1.0.0 "release" (func $release (;2;)))
  (core func $"#core-func6 indirect-cm32p2|runtara:workflow/waits@1-release" (@name "indirect-cm32p2|runtara:workflow/waits@1-release") (;6;) (canon lower (func $release) (memory $memory) string-encoding=utf8))
  (core instance $fixup-args (;3;)
    (export "$imports" (table $"shim table"))
    (export "0" (func $"#core-func4 indirect-cm32p2|runtara:workflow/waits@1-register"))
    (export "1" (func $"#core-func5 indirect-cm32p2|runtara:workflow/waits@1-poll"))
    (export "2" (func $"#core-func6 indirect-cm32p2|runtara:workflow/waits@1-release"))
  )
  (core instance $fixup (;4;) (instantiate $wit-component-fixup
      (with "" (instance $fixup-args))
    )
  )
  (alias core export $main "cm32p2_initialize" (core func $start (;7;)))
  (core module $start-shim-module (;3;)
    (type (;0;) (func))
    (import "" "" (func (;0;) (type 0)))
    (start 0)
  )
  (core instance $start-shim-args (;5;)
    (export "" (func $start))
  )
  (core instance $start-shim-instance (;6;) (instantiate $start-shim-module
      (with "" (instance $start-shim-args))
    )
  )
  (@producers
    (processed-by "wit-component" "0.249.0")
  )
)
