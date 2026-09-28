;; Frozen ABI fixture: a guest compiled against the released 0.1.0 WIT
;; (wit-component dummy guest, printed by wasm-tools 1.249.0). Never edit or
;; regenerate this file: it must keep linking for as long as the host runs
;; artifacts built against 0.1.0. See runtara-workflow-wit's ABI rule.
(component
  (type $ty-runtara:agent/types@0.4.0 (;0;)
    (instance
      (type (;0;) (option u64))
      (type (;1;) (option string))
      (type (;2;) (record (field "code" string) (field "message" string) (field "category" string) (field "severity" string) (field "retryable" bool) (field "retry-after-ms" 0) (field "attributes" 1)))
      (export (;3;) "error-info" (type (eq 2)))
    )
  )
  (import "runtara:agent/types@0.4.0" (instance $runtara:agent/types@0.4.0 (;0;) (type $ty-runtara:agent/types@0.4.0)))
  (alias export $runtara:agent/types@0.4.0 "error-info" (type $error-info (;1;)))
  (type $ty-runtara:control/executor@0.1.0 (;2;)
    (instance
      (alias outer 1 $error-info (type (;0;)))
      (export (;1;) "error-info" (type (eq 0)))
      (type (;2;) (list u8))
      (type (;3;) (result 2 (error 1)))
      (type (;4;) (func async (param "capability-id" string) (param "input" 2) (result 3)))
      (export (;0;) "invoke" (func (type 4)))
    )
  )
  (import "runtara:control/executor@0.1.0" (instance $runtara:control/executor@0.1.0 (;1;) (type $ty-runtara:control/executor@0.1.0)))
  (core module $main (;0;)
    (type (;0;) (func (param i32 i32 i32 i32 i32)))
    (type (;1;) (func (param i32 i32 i32 i32) (result i32)))
    (type (;2;) (func))
    (import "cm32p2|runtara:control/executor@0.1" "invoke" (func (;0;) (type 0)))
    (memory (;0;) 0)
    (export "cm32p2_memory" (memory 0))
    (export "cm32p2_realloc" (func 1))
    (export "cm32p2_initialize" (func 2))
    (func (;1;) (type 1) (param i32 i32 i32 i32) (result i32)
      unreachable
    )
    (func (;2;) (type 2))
    (@producers
      (processed-by "wit-component" "0.249.0")
    )
  )
  (core module $wit-component-shim-module (;1;)
    (type (;0;) (func (param i32 i32 i32 i32 i32)))
    (table (;0;) 1 1 funcref)
    (export "0" (func 0))
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
    (@producers
      (processed-by "wit-component" "0.249.0")
    )
  )
  (core module $wit-component-fixup (;2;)
    (type (;0;) (func (param i32 i32 i32 i32 i32)))
    (import "" "0" (func (;0;) (type 0)))
    (import "" "$imports" (table (;0;) 1 1 funcref))
    (elem (;0;) (i32.const 0) func 0)
    (@producers
      (processed-by "wit-component" "0.249.0")
    )
  )
  (core instance $wit-component-shim-instance (;0;) (instantiate $wit-component-shim-module))
  (alias core export $wit-component-shim-instance "0" (core func $indirect-cm32p2|runtara:control/executor@0.1-invoke (;0;)))
  (core instance $cm32p2|runtara:control/executor@0.1 (;1;)
    (export "invoke" (func $indirect-cm32p2|runtara:control/executor@0.1-invoke))
  )
  (core instance $main (;2;) (instantiate $main
      (with "cm32p2|runtara:control/executor@0.1" (instance $cm32p2|runtara:control/executor@0.1))
    )
  )
  (alias core export $main "cm32p2_memory" (core memory $memory (;0;)))
  (alias core export $wit-component-shim-instance "$imports" (core table $"shim table" (;0;)))
  (alias export $runtara:control/executor@0.1.0 "invoke" (func $invoke (;0;)))
  (alias core export $main "cm32p2_realloc" (core func $realloc (;1;)))
  (core func $"#core-func2 indirect-cm32p2|runtara:control/executor@0.1-invoke" (@name "indirect-cm32p2|runtara:control/executor@0.1-invoke") (;2;) (canon lower (func $invoke) (memory $memory) (realloc $realloc) string-encoding=utf8))
  (core instance $fixup-args (;3;)
    (export "$imports" (table $"shim table"))
    (export "0" (func $"#core-func2 indirect-cm32p2|runtara:control/executor@0.1-invoke"))
  )
  (core instance $fixup (;4;) (instantiate $wit-component-fixup
      (with "" (instance $fixup-args))
    )
  )
  (alias core export $main "cm32p2_initialize" (core func $start (;3;)))
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
