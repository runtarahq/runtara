;; Frozen ABI fixture: a guest compiled against the released 1.0.0 WIT
;; (wit-component dummy guest, printed by wasm-tools 1.249.0). Never edit or
;; regenerate this file: it must keep linking for as long as the host runs
;; artifacts built against 1.0.0. See runtara-wit's versioning rule.
(component
  (type $ty-runtara:control/types@1.0.0 (;0;)
    (instance
      (type (;0;) (enum "cancel" "leave-running"))
      (export (;1;) "parent-close-policy" (type (eq 0)))
      (type (;2;) (option u32))
      (type (;3;) (list u8))
      (type (;4;) (option string))
      (type (;5;) (record (field "workflow-id" string) (field "version" 2) (field "input" 3) (field "run-label" 4) (field "parent-close-policy" 1)))
      (export (;6;) "start-request" (type (eq 5)))
      (type (;7;) (record (field "instance-id" string) (field "workflow-id" string) (field "version" u32) (field "run-label" 4) (field "replayed" bool)))
      (export (;8;) "start-result" (type (eq 7)))
      (type (;9;) (enum "denied" "invalid" "not-found" "not-runnable" "not-child" "requires-instance" "requires-operation" "capacity" "replay-conflict" "label-conflict" "too-large" "unavailable" "unsupported" "not-waiting" "ambiguous" "already-answered" "not-pausable" "not-paused"))
      (export (;10;) "error-code" (type (eq 9)))
      (type (;11;) (option u64))
      (type (;12;) (record (field "code" 10) (field "message" string) (field "retry-after-ms" 11)))
      (export (;13;) "control-error" (type (eq 12)))
      (type (;14;) (enum "queued" "pending" "running" "suspended" "completed" "failed" "cancelled" "not-started"))
      (export (;15;) "instance-status" (type (eq 14)))
      (type (;16;) (enum "paused" "waiting-signal" "waiting-instances" "sleeping" "shutdown"))
      (export (;17;) "suspension-reason" (type (eq 16)))
      (type (;18;) (option 17))
      (type (;19;) (record (field "instance-id" string) (field "workflow-id" string) (field "version" 2) (field "run-label" 4) (field "parent-instance-id" 4) (field "status" 15) (field "suspension-reason" 18) (field "termination-reason" 4) (field "created-at-ms" u64) (field "started-at-ms" 11) (field "finished-at-ms" 11)))
      (export (;20;) "instance-summary" (type (eq 19)))
      (type (;21;) (option 3))
      (type (;22;) (record (field "output" 21) (field "output-bytes" 11) (field "output-omitted" bool) (field "error" 21) (field "error-omitted" bool)))
      (export (;23;) "terminal-result" (type (eq 22)))
      (type (;24;) (record (field "instance" 20) (field "terminal" 23)))
      (export (;25;) "instance-detail" (type (eq 24)))
      (type (;26;) (record (field "instance" 20) (field "state" 21) (field "state-updated-at-ms" 11)))
      (export (;27;) "state-read" (type (eq 26)))
      (type (;28;) (variant (case "caller") (case "instance" string)))
      (export (;29;) "parent-filter" (type (eq 28)))
      (type (;30;) (enum "created-at" "finished-at"))
      (export (;31;) "sort-field" (type (eq 30)))
      (type (;32;) (enum "ascending" "descending"))
      (export (;33;) "sort-order" (type (eq 32)))
      (type (;34;) (list 15))
      (type (;35;) (option 29))
      (type (;36;) (record (field "workflow-id" 4) (field "run-label" 4) (field "statuses" 34) (field "parent" 35) (field "created-after-ms" 11) (field "created-before-ms" 11) (field "finished-after-ms" 11) (field "finished-before-ms" 11) (field "state" 21) (field "sort-by" 31) (field "order" 33) (field "page-size" u32) (field "page-token" 4)))
      (export (;37;) "query-request" (type (eq 36)))
      (type (;38;) (list 20))
      (type (;39;) (record (field "items" 38) (field "total" u64) (field "next-page-token" 4)))
      (export (;40;) "instance-page" (type (eq 39)))
      (type (;41;) (variant (case "instance" string) (case "workflow" string) (case "children")))
      (export (;42;) "signal-scope" (type (eq 41)))
      (type (;43;) (record (field "scope" 42) (field "signal-id" 4) (field "action-key" 4) (field "page-size" u32) (field "page-token" 4)))
      (export (;44;) "pending-signals-request" (type (eq 43)))
      (type (;45;) (record (field "instance-id" string) (field "workflow-id" string) (field "signal-id" string) (field "request-id" string) (field "action-key" 4) (field "response-schema" 21) (field "context" 21) (field "requested-at-ms" u64) (field "deadline-ms" 11)))
      (export (;46;) "pending-signal" (type (eq 45)))
      (type (;47;) (list 46))
      (type (;48;) (record (field "items" 47) (field "next-page-token" 4)))
      (export (;49;) "pending-signal-page" (type (eq 48)))
      (type (;50;) (record (field "instance-id" string) (field "signal-id" string) (field "action-key" 4) (field "request-id" 4) (field "payload" 3)))
      (export (;51;) "send-signal-request" (type (eq 50)))
      (type (;52;) (record (field "request-id" string) (field "replayed" bool)))
      (export (;53;) "send-signal-result" (type (eq 52)))
      (type (;54;) (record (field "instance-id" string) (field "reason" 4) (field "grace-ms" 11)))
      (export (;55;) "cancel-request" (type (eq 54)))
      (type (;56;) (enum "requested" "applied" "unchanged" "already-terminal"))
      (export (;57;) "command-outcome" (type (eq 56)))
      (type (;58;) (record (field "instance-id" string) (field "outcome" 57) (field "replayed" bool)))
      (export (;59;) "command-result" (type (eq 58)))
    )
  )
  (import "runtara:control/types@1.0.0" (instance $runtara:control/types@1.0.0 (;0;) (type $ty-runtara:control/types@1.0.0)))
  (alias export $runtara:control/types@1.0.0 "start-request" (type $start-request (;1;)))
  (alias export $runtara:control/types@1.0.0 "start-result" (type $start-result (;2;)))
  (alias export $runtara:control/types@1.0.0 "control-error" (type $control-error (;3;)))
  (alias export $runtara:control/types@1.0.0 "instance-detail" (type $instance-detail (;4;)))
  (alias export $runtara:control/types@1.0.0 "state-read" (type $state-read (;5;)))
  (alias export $runtara:control/types@1.0.0 "query-request" (type $query-request (;6;)))
  (alias export $runtara:control/types@1.0.0 "instance-page" (type $instance-page (;7;)))
  (alias export $runtara:control/types@1.0.0 "pending-signals-request" (type $pending-signals-request (;8;)))
  (alias export $runtara:control/types@1.0.0 "pending-signal-page" (type $pending-signal-page (;9;)))
  (alias export $runtara:control/types@1.0.0 "send-signal-request" (type $send-signal-request (;10;)))
  (alias export $runtara:control/types@1.0.0 "send-signal-result" (type $send-signal-result (;11;)))
  (alias export $runtara:control/types@1.0.0 "cancel-request" (type $cancel-request (;12;)))
  (alias export $runtara:control/types@1.0.0 "command-result" (type $command-result (;13;)))
  (type $ty-runtara:control/api@1.0.0 (;14;)
    (instance
      (alias outer 1 $start-request (type (;0;)))
      (export (;1;) "start-request" (type (eq 0)))
      (alias outer 1 $start-result (type (;2;)))
      (export (;3;) "start-result" (type (eq 2)))
      (alias outer 1 $control-error (type (;4;)))
      (export (;5;) "control-error" (type (eq 4)))
      (alias outer 1 $instance-detail (type (;6;)))
      (export (;7;) "instance-detail" (type (eq 6)))
      (alias outer 1 $state-read (type (;8;)))
      (export (;9;) "state-read" (type (eq 8)))
      (alias outer 1 $query-request (type (;10;)))
      (export (;11;) "query-request" (type (eq 10)))
      (alias outer 1 $instance-page (type (;12;)))
      (export (;13;) "instance-page" (type (eq 12)))
      (alias outer 1 $pending-signals-request (type (;14;)))
      (export (;15;) "pending-signals-request" (type (eq 14)))
      (alias outer 1 $pending-signal-page (type (;16;)))
      (export (;17;) "pending-signal-page" (type (eq 16)))
      (alias outer 1 $send-signal-request (type (;18;)))
      (export (;19;) "send-signal-request" (type (eq 18)))
      (alias outer 1 $send-signal-result (type (;20;)))
      (export (;21;) "send-signal-result" (type (eq 20)))
      (alias outer 1 $cancel-request (type (;22;)))
      (export (;23;) "cancel-request" (type (eq 22)))
      (alias outer 1 $command-result (type (;24;)))
      (export (;25;) "command-result" (type (eq 24)))
      (type (;26;) (result 3 (error 5)))
      (type (;27;) (func async (param "request" 1) (result 26)))
      (export (;0;) "start" (func (type 27)))
      (type (;28;) (result 7 (error 5)))
      (type (;29;) (func async (param "instance-id" string) (result 28)))
      (export (;1;) "get" (func (type 29)))
      (type (;30;) (result 9 (error 5)))
      (type (;31;) (func async (param "instance-id" string) (result 30)))
      (export (;2;) "get-state" (func (type 31)))
      (type (;32;) (result 13 (error 5)))
      (type (;33;) (func async (param "request" 11) (result 32)))
      (export (;3;) "query" (func (type 33)))
      (type (;34;) (result 17 (error 5)))
      (type (;35;) (func async (param "request" 15) (result 34)))
      (export (;4;) "list-pending-signals" (func (type 35)))
      (type (;36;) (result 21 (error 5)))
      (type (;37;) (func async (param "request" 19) (result 36)))
      (export (;5;) "send-signal" (func (type 37)))
      (type (;38;) (result 25 (error 5)))
      (type (;39;) (func async (param "request" 23) (result 38)))
      (export (;6;) "cancel" (func (type 39)))
      (type (;40;) (func async (param "instance-id" string) (result 38)))
      (export (;7;) "pause" (func (type 40)))
      (export (;8;) "resume" (func (type 40)))
    )
  )
  (import "runtara:control/api@1.0.0" (instance $runtara:control/api@1.0.0 (;1;) (type $ty-runtara:control/api@1.0.0)))
  (core module $main (;0;)
    (type (;0;) (func (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)))
    (type (;1;) (func (param i32 i32 i32)))
    (type (;2;) (func (param i32 i32)))
    (type (;3;) (func (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)))
    (type (;4;) (func (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)))
    (type (;5;) (func (param i32 i32 i32 i32 i32 i32 i64 i32)))
    (type (;6;) (func (param i32 i32 i32 i32) (result i32)))
    (type (;7;) (func))
    (import "cm32p2|runtara:control/api@1" "start" (func (;0;) (type 0)))
    (import "cm32p2|runtara:control/api@1" "get" (func (;1;) (type 1)))
    (import "cm32p2|runtara:control/api@1" "get-state" (func (;2;) (type 1)))
    (import "cm32p2|runtara:control/api@1" "query" (func (;3;) (type 2)))
    (import "cm32p2|runtara:control/api@1" "list-pending-signals" (func (;4;) (type 3)))
    (import "cm32p2|runtara:control/api@1" "send-signal" (func (;5;) (type 4)))
    (import "cm32p2|runtara:control/api@1" "cancel" (func (;6;) (type 5)))
    (import "cm32p2|runtara:control/api@1" "pause" (func (;7;) (type 1)))
    (import "cm32p2|runtara:control/api@1" "resume" (func (;8;) (type 1)))
    (memory (;0;) 0)
    (export "cm32p2_memory" (memory 0))
    (export "cm32p2_realloc" (func 9))
    (export "cm32p2_initialize" (func 10))
    (func (;9;) (type 6) (param i32 i32 i32 i32) (result i32)
      unreachable
    )
    (func (;10;) (type 7))
    (@producers
      (processed-by "wit-component" "0.249.0")
    )
  )
  (core module $wit-component-shim-module (;1;)
    (type (;0;) (func (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)))
    (type (;1;) (func (param i32 i32 i32)))
    (type (;2;) (func (param i32 i32)))
    (type (;3;) (func (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)))
    (type (;4;) (func (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)))
    (type (;5;) (func (param i32 i32 i32 i32 i32 i32 i64 i32)))
    (table (;0;) 9 9 funcref)
    (export "0" (func 0))
    (export "1" (func 1))
    (export "2" (func 2))
    (export "3" (func 3))
    (export "4" (func 4))
    (export "5" (func 5))
    (export "6" (func 6))
    (export "7" (func 7))
    (export "8" (func 8))
    (export "$imports" (table 0))
    (func (;0;) (type 0) (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)
      local.get 0
      local.get 1
      local.get 2
      local.get 3
      local.get 4
      local.get 5
      local.get 6
      local.get 7
      local.get 8
      local.get 9
      local.get 10
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
    (func (;2;) (type 1) (param i32 i32 i32)
      local.get 0
      local.get 1
      local.get 2
      i32.const 2
      call_indirect (type 1)
    )
    (func (;3;) (type 2) (param i32 i32)
      local.get 0
      local.get 1
      i32.const 3
      call_indirect (type 2)
    )
    (func (;4;) (type 3) (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)
      local.get 0
      local.get 1
      local.get 2
      local.get 3
      local.get 4
      local.get 5
      local.get 6
      local.get 7
      local.get 8
      local.get 9
      local.get 10
      local.get 11
      local.get 12
      local.get 13
      i32.const 4
      call_indirect (type 3)
    )
    (func (;5;) (type 4) (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)
      local.get 0
      local.get 1
      local.get 2
      local.get 3
      local.get 4
      local.get 5
      local.get 6
      local.get 7
      local.get 8
      local.get 9
      local.get 10
      local.get 11
      local.get 12
      i32.const 5
      call_indirect (type 4)
    )
    (func (;6;) (type 5) (param i32 i32 i32 i32 i32 i32 i64 i32)
      local.get 0
      local.get 1
      local.get 2
      local.get 3
      local.get 4
      local.get 5
      local.get 6
      local.get 7
      i32.const 6
      call_indirect (type 5)
    )
    (func (;7;) (type 1) (param i32 i32 i32)
      local.get 0
      local.get 1
      local.get 2
      i32.const 7
      call_indirect (type 1)
    )
    (func (;8;) (type 1) (param i32 i32 i32)
      local.get 0
      local.get 1
      local.get 2
      i32.const 8
      call_indirect (type 1)
    )
    (@producers
      (processed-by "wit-component" "0.249.0")
    )
  )
  (core module $wit-component-fixup (;2;)
    (type (;0;) (func (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)))
    (type (;1;) (func (param i32 i32 i32)))
    (type (;2;) (func (param i32 i32)))
    (type (;3;) (func (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)))
    (type (;4;) (func (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)))
    (type (;5;) (func (param i32 i32 i32 i32 i32 i32 i64 i32)))
    (import "" "0" (func (;0;) (type 0)))
    (import "" "1" (func (;1;) (type 1)))
    (import "" "2" (func (;2;) (type 1)))
    (import "" "3" (func (;3;) (type 2)))
    (import "" "4" (func (;4;) (type 3)))
    (import "" "5" (func (;5;) (type 4)))
    (import "" "6" (func (;6;) (type 5)))
    (import "" "7" (func (;7;) (type 1)))
    (import "" "8" (func (;8;) (type 1)))
    (import "" "$imports" (table (;0;) 9 9 funcref))
    (elem (;0;) (i32.const 0) func 0 1 2 3 4 5 6 7 8)
    (@producers
      (processed-by "wit-component" "0.249.0")
    )
  )
  (core instance $wit-component-shim-instance (;0;) (instantiate $wit-component-shim-module))
  (alias core export $wit-component-shim-instance "0" (core func $indirect-cm32p2|runtara:control/api@1-start (;0;)))
  (alias core export $wit-component-shim-instance "1" (core func $indirect-cm32p2|runtara:control/api@1-get (;1;)))
  (alias core export $wit-component-shim-instance "2" (core func $indirect-cm32p2|runtara:control/api@1-get-state (;2;)))
  (alias core export $wit-component-shim-instance "3" (core func $indirect-cm32p2|runtara:control/api@1-query (;3;)))
  (alias core export $wit-component-shim-instance "4" (core func $indirect-cm32p2|runtara:control/api@1-list-pending-signals (;4;)))
  (alias core export $wit-component-shim-instance "5" (core func $indirect-cm32p2|runtara:control/api@1-send-signal (;5;)))
  (alias core export $wit-component-shim-instance "6" (core func $indirect-cm32p2|runtara:control/api@1-cancel (;6;)))
  (alias core export $wit-component-shim-instance "7" (core func $indirect-cm32p2|runtara:control/api@1-pause (;7;)))
  (alias core export $wit-component-shim-instance "8" (core func $indirect-cm32p2|runtara:control/api@1-resume (;8;)))
  (core instance $cm32p2|runtara:control/api@1 (;1;)
    (export "start" (func $indirect-cm32p2|runtara:control/api@1-start))
    (export "get" (func $indirect-cm32p2|runtara:control/api@1-get))
    (export "get-state" (func $indirect-cm32p2|runtara:control/api@1-get-state))
    (export "query" (func $indirect-cm32p2|runtara:control/api@1-query))
    (export "list-pending-signals" (func $indirect-cm32p2|runtara:control/api@1-list-pending-signals))
    (export "send-signal" (func $indirect-cm32p2|runtara:control/api@1-send-signal))
    (export "cancel" (func $indirect-cm32p2|runtara:control/api@1-cancel))
    (export "pause" (func $indirect-cm32p2|runtara:control/api@1-pause))
    (export "resume" (func $indirect-cm32p2|runtara:control/api@1-resume))
  )
  (core instance $main (;2;) (instantiate $main
      (with "cm32p2|runtara:control/api@1" (instance $cm32p2|runtara:control/api@1))
    )
  )
  (alias core export $main "cm32p2_memory" (core memory $memory (;0;)))
  (alias core export $wit-component-shim-instance "$imports" (core table $"shim table" (;0;)))
  (alias export $runtara:control/api@1.0.0 "start" (func $start (;0;)))
  (alias core export $main "cm32p2_realloc" (core func $realloc (;9;)))
  (core func $"#core-func10 indirect-cm32p2|runtara:control/api@1-start" (@name "indirect-cm32p2|runtara:control/api@1-start") (;10;) (canon lower (func $start) (memory $memory) (realloc $realloc) string-encoding=utf8))
  (alias export $runtara:control/api@1.0.0 "get" (func $get (;1;)))
  (core func $"#core-func11 indirect-cm32p2|runtara:control/api@1-get" (@name "indirect-cm32p2|runtara:control/api@1-get") (;11;) (canon lower (func $get) (memory $memory) (realloc $realloc) string-encoding=utf8))
  (alias export $runtara:control/api@1.0.0 "get-state" (func $get-state (;2;)))
  (core func $"#core-func12 indirect-cm32p2|runtara:control/api@1-get-state" (@name "indirect-cm32p2|runtara:control/api@1-get-state") (;12;) (canon lower (func $get-state) (memory $memory) (realloc $realloc) string-encoding=utf8))
  (alias export $runtara:control/api@1.0.0 "query" (func $query (;3;)))
  (core func $"#core-func13 indirect-cm32p2|runtara:control/api@1-query" (@name "indirect-cm32p2|runtara:control/api@1-query") (;13;) (canon lower (func $query) (memory $memory) (realloc $realloc) string-encoding=utf8))
  (alias export $runtara:control/api@1.0.0 "list-pending-signals" (func $list-pending-signals (;4;)))
  (core func $"#core-func14 indirect-cm32p2|runtara:control/api@1-list-pending-signals" (@name "indirect-cm32p2|runtara:control/api@1-list-pending-signals") (;14;) (canon lower (func $list-pending-signals) (memory $memory) (realloc $realloc) string-encoding=utf8))
  (alias export $runtara:control/api@1.0.0 "send-signal" (func $send-signal (;5;)))
  (core func $"#core-func15 indirect-cm32p2|runtara:control/api@1-send-signal" (@name "indirect-cm32p2|runtara:control/api@1-send-signal") (;15;) (canon lower (func $send-signal) (memory $memory) (realloc $realloc) string-encoding=utf8))
  (alias export $runtara:control/api@1.0.0 "cancel" (func $cancel (;6;)))
  (core func $"#core-func16 indirect-cm32p2|runtara:control/api@1-cancel" (@name "indirect-cm32p2|runtara:control/api@1-cancel") (;16;) (canon lower (func $cancel) (memory $memory) (realloc $realloc) string-encoding=utf8))
  (alias export $runtara:control/api@1.0.0 "pause" (func $pause (;7;)))
  (core func $"#core-func17 indirect-cm32p2|runtara:control/api@1-pause" (@name "indirect-cm32p2|runtara:control/api@1-pause") (;17;) (canon lower (func $pause) (memory $memory) (realloc $realloc) string-encoding=utf8))
  (alias export $runtara:control/api@1.0.0 "resume" (func $resume (;8;)))
  (core func $"#core-func18 indirect-cm32p2|runtara:control/api@1-resume" (@name "indirect-cm32p2|runtara:control/api@1-resume") (;18;) (canon lower (func $resume) (memory $memory) (realloc $realloc) string-encoding=utf8))
  (core instance $fixup-args (;3;)
    (export "$imports" (table $"shim table"))
    (export "0" (func $"#core-func10 indirect-cm32p2|runtara:control/api@1-start"))
    (export "1" (func $"#core-func11 indirect-cm32p2|runtara:control/api@1-get"))
    (export "2" (func $"#core-func12 indirect-cm32p2|runtara:control/api@1-get-state"))
    (export "3" (func $"#core-func13 indirect-cm32p2|runtara:control/api@1-query"))
    (export "4" (func $"#core-func14 indirect-cm32p2|runtara:control/api@1-list-pending-signals"))
    (export "5" (func $"#core-func15 indirect-cm32p2|runtara:control/api@1-send-signal"))
    (export "6" (func $"#core-func16 indirect-cm32p2|runtara:control/api@1-cancel"))
    (export "7" (func $"#core-func17 indirect-cm32p2|runtara:control/api@1-pause"))
    (export "8" (func $"#core-func18 indirect-cm32p2|runtara:control/api@1-resume"))
  )
  (core instance $fixup (;4;) (instantiate $wit-component-fixup
      (with "" (instance $fixup-args))
    )
  )
  (alias core export $main "cm32p2_initialize" (core func $start (;19;)))
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
