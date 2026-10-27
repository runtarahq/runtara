// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Asyncify for the emitter's own core modules.
//!
//! A callback-lifted export must return to the component-model event loop
//! at every wait, and resume where it stopped when the loop calls back. This
//! pass instruments a core module so that a call to one of its *leaf*
//! functions can unwind the whole call stack into a buffer and later rewind
//! it. The leaves (the emitter's wait primitives) and the *drivers* (the
//! export and callback that call into the body) are written by the emitter
//! and manage the state themselves; the pass instruments every other
//! function that can reach a leaf.
//!
//! The state lives in two globals the emitter defines: `state` (normal,
//! unwinding, rewinding) and `data`, the address of the buffer header
//! `[cursor: i32, end: i32]`. An instrumented function saves, when its call
//! unwinds, a frame of the index of that call and all of its locals; on
//! entry while rewinding it pops its frame and restores them. Frames are
//! pushed innermost first, so a rewind pops the outermost first.
//!
//! Unlike Binaryen's asyncify, the pass does not flatten the code. It splits
//! each sequence into statements (from one point where the operand stack is
//! at the sequence's base to the next) and, in a statement that can unwind,
//! keeps the values on the operand stack in locals across every call or
//! construct that can unwind, so they are saved with the frame. While
//! rewinding, every run of code that cannot unwind is skipped
//! (`if (state == normal)`), each call that can runs only when it is the one
//! that unwound (`if (normal || resume == k)`), and an `if` takes the arm that
//! holds the resume index. Branches are renumbered around the blocks the pass
//! inserts. A `block`, `loop` or `if` that holds a call that can unwind must
//! take no parameters and have no results; anything else the pass refuses.

// Only the tests call the pass until published workflow-agents are emitted
// with the callback lift (SYN-770).
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet};

use wasm_encoder::reencode::{Reencode, RoundtripReencoder};
use wasm_encoder::{BlockType as EncBlockType, Encode, Function, Instruction, MemArg};
use wasmparser::{
    BlockType, FuncValidatorAllocations, FunctionBody, Operator, Parser, Payload, ValType,
    ValidPayload, Validator, WasmFeatures,
};

// `state` is 0 while running normally (tested with `i32.eqz`).
/// `state`: a leaf asked to unwind; every frame saves itself and returns.
pub(crate) const UNWINDING: i32 = 1;
/// `state`: frames restore themselves until the leaf is reached again.
pub(crate) const REWINDING: i32 = 2;

/// What to instrument.
#[derive(Debug, Clone)]
pub(crate) struct AsyncifyConfig {
    /// Functions that unwind and rewind (they manage `state` themselves).
    pub(crate) leaves: BTreeSet<u32>,
    /// Functions that call into the body and are never instrumented.
    pub(crate) drivers: BTreeSet<u32>,
    /// The mutable i32 global holding the state.
    pub(crate) state_global: u32,
    /// The mutable i32 global holding the buffer header's address.
    pub(crate) data_global: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AsyncifyError(pub(crate) String);

impl std::fmt::Display for AsyncifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "asyncify: {}", self.0)
    }
}

impl std::error::Error for AsyncifyError {}

fn error(message: impl Into<String>) -> AsyncifyError {
    AsyncifyError(message.into())
}

/// The instrumented module, and the functions instrumented.
pub(crate) fn asyncify(
    module: &[u8],
    config: &AsyncifyConfig,
) -> Result<(Vec<u8>, BTreeSet<u32>), AsyncifyError> {
    let shape = Shape::read(module)?;
    let instrumented = shape.instrumented(config)?;
    let mut bodies = BTreeMap::new();
    let mut validator = Validator::new_with_features(WasmFeatures::all());
    let mut index = shape.imported_functions;
    for payload in Parser::new(0).parse_all(module) {
        let payload = payload.map_err(|e| error(e.to_string()))?;
        if let ValidPayload::Func(func, body) = validator
            .payload(&payload)
            .map_err(|e| error(e.to_string()))?
        {
            if instrumented.contains(&index) {
                let validator = func.into_validator(FuncValidatorAllocations::default());
                let function = Instrument::new(&shape, config, &instrumented, index)
                    .run(validator, &body)
                    .map_err(|e| error(format!("function {index}: {}", e.0)))?;
                bodies.insert(index, function);
            }
            index += 1;
        }
    }
    let mut reencoder = Splice {
        bodies,
        next: shape.imported_functions,
    };
    let mut out = wasm_encoder::Module::new();
    reencoder
        .parse_core_module(&mut out, Parser::new(0), module)
        .map_err(|e| error(format!("{e:?}")))?;
    Ok((out.finish(), instrumented))
}

/// Writes the module back, with instrumented bodies in place of theirs.
struct Splice {
    bodies: BTreeMap<u32, Function>,
    next: u32,
}

impl Reencode for Splice {
    type Error = std::convert::Infallible;

    fn parse_function_body(
        &mut self,
        code: &mut wasm_encoder::CodeSection,
        func: FunctionBody<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error<Self::Error>> {
        let index = self.next;
        self.next += 1;
        match self.bodies.remove(&index) {
            Some(function) => {
                code.function(&function);
                Ok(())
            }
            None => wasm_encoder::reencode::utils::parse_function_body(self, code, func),
        }
    }
}

/// The module's function signatures and direct call graph.
struct Shape {
    types: Vec<(Vec<ValType>, Vec<ValType>)>,
    function_types: Vec<u32>,
    imported_functions: u32,
    callees: BTreeMap<u32, BTreeSet<u32>>,
    table_functions: BTreeSet<u32>,
}

impl Shape {
    fn read(module: &[u8]) -> Result<Self, AsyncifyError> {
        let wrap = |e: wasmparser::BinaryReaderError| error(e.to_string());
        let mut shape = Shape {
            types: Vec::new(),
            function_types: Vec::new(),
            imported_functions: 0,
            callees: BTreeMap::new(),
            table_functions: BTreeSet::new(),
        };
        let mut defined = 0u32;
        for payload in Parser::new(0).parse_all(module) {
            match payload.map_err(wrap)? {
                Payload::TypeSection(reader) => {
                    for ty in reader.into_iter_err_on_gc_types() {
                        let ty = ty.map_err(wrap)?;
                        shape
                            .types
                            .push((ty.params().to_vec(), ty.results().to_vec()));
                    }
                }
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        if let wasmparser::TypeRef::Func(ty) = import.map_err(wrap)?.ty {
                            shape.function_types.push(ty);
                            shape.imported_functions += 1;
                        }
                    }
                }
                Payload::FunctionSection(reader) => {
                    for ty in reader {
                        shape.function_types.push(ty.map_err(wrap)?);
                    }
                }
                Payload::ElementSection(reader) => {
                    for element in reader {
                        match element.map_err(wrap)?.items {
                            wasmparser::ElementItems::Functions(functions) => {
                                for function in functions {
                                    shape.table_functions.insert(function.map_err(wrap)?);
                                }
                            }
                            wasmparser::ElementItems::Expressions(..) => {
                                return Err(error("element expressions are not supported"));
                            }
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    let index = shape.imported_functions + defined;
                    defined += 1;
                    let mut callees = BTreeSet::new();
                    for op in body.get_operators_reader().map_err(wrap)? {
                        match op.map_err(wrap)? {
                            Operator::Call { function_index } => {
                                callees.insert(function_index);
                            }
                            Operator::ReturnCall { function_index } => {
                                // A tail call cannot save its frame.
                                callees.insert(function_index);
                                callees.insert(u32::MAX);
                            }
                            _ => {}
                        }
                    }
                    shape.callees.insert(index, callees);
                }
                _ => {}
            }
        }
        Ok(shape)
    }

    fn signature(&self, function: u32) -> Result<&(Vec<ValType>, Vec<ValType>), AsyncifyError> {
        let ty = self
            .function_types
            .get(function as usize)
            .ok_or_else(|| error(format!("no function {function}")))?;
        self.types
            .get(*ty as usize)
            .ok_or_else(|| error(format!("no type {ty}")))
    }

    /// Every function, apart from leaves and drivers, that can reach a leaf.
    fn instrumented(&self, config: &AsyncifyConfig) -> Result<BTreeSet<u32>, AsyncifyError> {
        let mut reaches: BTreeSet<u32> = config.leaves.clone();
        loop {
            let before = reaches.len();
            for (caller, callees) in &self.callees {
                if !config.drivers.contains(caller)
                    && !config.leaves.contains(caller)
                    && callees.iter().any(|callee| reaches.contains(callee))
                {
                    reaches.insert(*caller);
                }
            }
            if reaches.len() == before {
                break;
            }
        }
        let instrumented: BTreeSet<u32> = reaches.difference(&config.leaves).copied().collect();
        for function in &instrumented {
            if self.callees[function].contains(&u32::MAX) {
                return Err(error(format!("function {function} tail-calls")));
            }
        }
        if let Some(function) = self.table_functions.iter().find(|f| reaches.contains(f)) {
            return Err(error(format!(
                "function {function} can unwind and is in a table"
            )));
        }
        Ok(instrumented)
    }
}

/// A parsed body: operators and the constructs holding them.
enum Node<'a> {
    Op {
        op: Operator<'a>,
        /// Operand-stack height after the operator.
        after: u32,
        /// The operand stack's types before and after the operator.
        stack: Vec<ValType>,
        stack_after: Vec<ValType>,
        /// It cannot run: it follows an unconditional branch in its block,
        /// or sits in a construct that does.
        dead: bool,
        /// For a live call that can unwind: its index in program order.
        call: Option<i32>,
        /// Byte offset in the module, for errors.
        offset: usize,
    },
    Construct(Box<Construct<'a>>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Block,
    Loop,
    If,
}

struct Construct<'a> {
    kind: Kind,
    ty: BlockType,
    body: Vec<Node<'a>>,
    els: Option<Vec<Node<'a>>>,
    /// Height before the construct's operator (an `if`'s includes its
    /// condition), and the stack's types there.
    before: u32,
    stack: Vec<ValType>,
    after: u32,
    dead: bool,
}

impl Node<'_> {
    fn after(&self) -> u32 {
        match self {
            Node::Op { after, .. } => *after,
            Node::Construct(c) => c.after,
        }
    }

    fn dead(&self) -> bool {
        match self {
            Node::Op { dead, .. } => *dead,
            Node::Construct(c) => c.dead,
        }
    }

    fn stack(&self) -> &[ValType] {
        match self {
            Node::Op { stack, .. } => stack,
            Node::Construct(c) => &c.stack,
        }
    }
}

/// One function's instrumentation.
struct Instrument<'s> {
    shape: &'s Shape,
    config: &'s AsyncifyConfig,
    instrumented: &'s BTreeSet<u32>,
    function: u32,
    /// The function's own locals (parameters first).
    locals: Vec<ValType>,
    resume: u32,
    fp: u32,
    /// Locals holding the operand stack across a call that can unwind, by
    /// stack position and type; declared after `resume` and `fp`.
    spills: BTreeMap<(u32, u8), u32>,
    spill_types: Vec<ValType>,
    frame: u32,
    /// The first emission only allocates spill locals.
    dry: bool,
    /// Original (true) and inserted (false) labels, innermost last.
    labels: Vec<bool>,
    /// The encoded body.
    out: Vec<u8>,
}

/// A statement of a sequence: operators and constructs from one point where
/// the operand stack is at the sequence's base to the next.
struct Statement<'n, 'a> {
    nodes: &'n [Node<'a>],
}

impl<'s> Instrument<'s> {
    fn new(
        shape: &'s Shape,
        config: &'s AsyncifyConfig,
        instrumented: &'s BTreeSet<u32>,
        function: u32,
    ) -> Self {
        Self {
            shape,
            config,
            instrumented,
            function,
            locals: Vec::new(),
            resume: 0,
            fp: 0,
            spills: BTreeMap::new(),
            spill_types: Vec::new(),
            frame: 0,
            dry: false,
            labels: Vec::new(),
            out: Vec::new(),
        }
    }

    fn unwinds(&self, function: u32) -> bool {
        self.instrumented.contains(&function) || self.config.leaves.contains(&function)
    }

    fn run(
        mut self,
        mut validator: wasmparser::FuncValidator<wasmparser::ValidatorResources>,
        body: &FunctionBody<'_>,
    ) -> Result<Function, AsyncifyError> {
        let wrap = |e: wasmparser::BinaryReaderError| error(e.to_string());
        let (params, results) = self.shape.signature(self.function)?.clone();
        self.locals = params;
        let mut declared = Vec::new();
        let mut locals = body.get_locals_reader().map_err(wrap)?;
        for _ in 0..locals.get_count() {
            let offset = locals.original_position();
            let (count, ty) = locals.read().map_err(wrap)?;
            validator.define_locals(offset, count, ty).map_err(wrap)?;
            declared.push((count, ty));
            for _ in 0..count {
                self.locals.push(ty);
            }
        }
        let original = self.locals.len() as u32;
        (self.resume, self.fp) = (original, original + 1);
        let unwinds = |function: u32| self.unwinds(function);
        let tree = parse(validator, body, &unwinds)?;

        // Once to learn the spill locals, which the frame holds, then again.
        self.dry = true;
        self.labels = vec![true];
        self.sequence(&tree, 0, false, &results)?;
        let spills = self.spills.len();
        self.dry = false;
        self.frame = 4 + self
            .saved()
            .iter()
            .map(|(_, ty)| size_of(*ty))
            .sum::<Result<u32, _>>()?;
        self.labels = vec![true];
        self.prologue()?;
        self.sequence(&tree, 0, false, &results)?;
        self.emit(Instruction::End);
        debug_assert_eq!(spills, self.spills.len(), "both emissions spill alike");

        let mut encoder = RoundtripReencoder;
        let mut groups = Vec::new();
        for (count, ty) in declared {
            groups.push((
                count,
                encoder.val_type(ty).map_err(|e| error(format!("{e:?}")))?,
            ));
        }
        groups.push((2, wasm_encoder::ValType::I32));
        for ty in self.spill_types.clone() {
            groups.push((
                1,
                encoder.val_type(ty).map_err(|e| error(format!("{e:?}")))?,
            ));
        }
        let mut function = Function::new(groups);
        function.raw(self.out);
        Ok(function)
    }

    /// The locals a frame keeps: the function's own, then the spills.
    fn saved(&self) -> Vec<(u32, ValType)> {
        let own = self
            .locals
            .iter()
            .enumerate()
            .map(|(n, ty)| (n as u32, *ty));
        let first_spill = self.fp + 1;
        let spills = self
            .spill_types
            .iter()
            .enumerate()
            .map(|(n, ty)| (first_spill + n as u32, *ty));
        own.chain(spills).collect()
    }

    /// The local holding stack position `position` of type `ty`.
    fn spill(&mut self, position: u32, ty: ValType) -> Result<u32, AsyncifyError> {
        let code = match ty {
            ValType::I32 => 0,
            ValType::I64 => 1,
            ValType::F32 => 2,
            ValType::F64 => 3,
            other => {
                return Err(error(format!(
                    "a stack value of type {other:?} cannot be kept"
                )));
            }
        };
        if let Some(local) = self.spills.get(&(position, code)) {
            return Ok(*local);
        }
        let local = self.fp + 1 + self.spill_types.len() as u32;
        self.spills.insert((position, code), local);
        self.spill_types.push(ty);
        Ok(local)
    }

    fn emit(&mut self, instruction: Instruction<'_>) {
        if !self.dry {
            instruction.encode(&mut self.out);
        }
    }

    fn state(&mut self) {
        self.emit(Instruction::GlobalGet(self.config.state_global));
    }

    fn data_header(&mut self) {
        self.emit(Instruction::GlobalGet(self.config.data_global));
    }

    /// While rewinding, pop this function's frame and restore its locals.
    fn prologue(&mut self) -> Result<(), AsyncifyError> {
        self.state();
        self.emit(Instruction::I32Const(REWINDING));
        self.emit(Instruction::I32Eq);
        self.emit(Instruction::If(EncBlockType::Empty));
        self.data_header();
        self.data_header();
        self.emit(Instruction::I32Load(mem32(0)));
        self.emit(Instruction::I32Const(self.frame as i32));
        self.emit(Instruction::I32Sub);
        self.emit(Instruction::I32Store(mem32(0)));
        self.data_header();
        self.emit(Instruction::I32Load(mem32(0)));
        self.emit(Instruction::LocalSet(self.fp));
        self.emit(Instruction::LocalGet(self.fp));
        self.emit(Instruction::I32Load(mem32(0)));
        self.emit(Instruction::LocalSet(self.resume));
        let mut offset = 4u64;
        for (local, ty) in self.saved() {
            self.emit(Instruction::LocalGet(self.fp));
            self.emit(load(ty, offset)?);
            self.emit(Instruction::LocalSet(local));
            offset += u64::from(size_of(ty)?);
        }
        self.emit(Instruction::End);
        Ok(())
    }

    /// Push this function's frame for the call at `resume` and return.
    fn unwind(&mut self, results: &[ValType]) -> Result<(), AsyncifyError> {
        self.data_header();
        self.emit(Instruction::I32Load(mem32(0)));
        self.emit(Instruction::LocalSet(self.fp));
        // A full buffer cannot keep the stack.
        self.emit(Instruction::LocalGet(self.fp));
        self.emit(Instruction::I32Const(self.frame as i32));
        self.emit(Instruction::I32Add);
        self.data_header();
        self.emit(Instruction::I32Load(mem32(4)));
        self.emit(Instruction::I32GtU);
        self.emit(Instruction::If(EncBlockType::Empty));
        self.emit(Instruction::Unreachable);
        self.emit(Instruction::End);
        self.emit(Instruction::LocalGet(self.fp));
        self.emit(Instruction::LocalGet(self.resume));
        self.emit(Instruction::I32Store(mem32(0)));
        let mut offset = 4u64;
        for (local, ty) in self.saved() {
            self.emit(Instruction::LocalGet(self.fp));
            self.emit(Instruction::LocalGet(local));
            self.emit(store(ty, offset)?);
            offset += u64::from(size_of(ty)?);
        }
        self.data_header();
        self.emit(Instruction::LocalGet(self.fp));
        self.emit(Instruction::I32Const(self.frame as i32));
        self.emit(Instruction::I32Add);
        self.emit(Instruction::I32Store(mem32(0)));
        for ty in results {
            self.emit(zero(*ty)?);
        }
        self.emit(Instruction::Return);
        Ok(())
    }

    fn sequence(
        &mut self,
        nodes: &[Node<'_>],
        base: u32,
        nested: bool,
        results: &[ValType],
    ) -> Result<(), AsyncifyError> {
        let mut run: Vec<&Node<'_>> = Vec::new();
        let statements = statements(nodes, base);
        let last = statements.len().saturating_sub(1);
        for (n, statement) in statements.iter().enumerate() {
            if !self.statement_unwinds(statement) {
                run.extend(statement.nodes);
                continue;
            }
            self.flush(&mut run, true)?;
            // The function's last statement may leave its results; nothing
            // after the call that unwound runs while rewinding.
            let leaves_results = !nested && n == last;
            self.unwinding_statement(statement.nodes, base, leaves_results, results)?;
        }
        self.flush(&mut run, nested)
    }

    fn statement_unwinds(&self, statement: &Statement<'_, '_>) -> bool {
        statement.nodes.iter().any(|node| self.node_unwinds(node))
    }

    fn node_unwinds(&self, node: &Node<'_>) -> bool {
        if node.dead() {
            return false;
        }
        match node {
            Node::Op { call, .. } => call.is_some(),
            Node::Construct(c) => {
                c.body.iter().any(|n| self.node_unwinds(n))
                    || c.els
                        .as_ref()
                        .is_some_and(|els| els.iter().any(|n| self.node_unwinds(n)))
            }
        }
    }

    /// Emit a run of statements that cannot unwind, skipped while rewinding
    /// when `guard`.
    fn flush(&mut self, run: &mut Vec<&Node<'_>>, guard: bool) -> Result<(), AsyncifyError> {
        if run.is_empty() {
            return Ok(());
        }
        if guard {
            self.state();
            self.emit(Instruction::I32Eqz);
            self.emit(Instruction::If(EncBlockType::Empty));
            self.labels.push(false);
        }
        for node in run.drain(..) {
            self.verbatim(node)?;
        }
        if guard {
            self.labels.pop();
            self.emit(Instruction::End);
        }
        Ok(())
    }

    /// A statement holding calls or constructs that can unwind. The operand
    /// stack above `base` lives in spill locals across each of them (its
    /// image); the code between them is skipped while rewinding.
    fn unwinding_statement(
        &mut self,
        nodes: &[Node<'_>],
        base: u32,
        leaves_results: bool,
        results: &[ValType],
    ) -> Result<(), AsyncifyError> {
        let mut image: Vec<ValType> = Vec::new();
        let mut run: Vec<&Node<'_>> = Vec::new();
        for node in nodes {
            if !self.node_unwinds(node) {
                run.push(node);
                continue;
            }
            let target = node.stack()[base as usize..].to_vec();
            self.spilled_run(&image, &run, &target, base, true)?;
            run.clear();
            image = target;
            match node {
                Node::Op {
                    call: Some(k),
                    stack_after,
                    ..
                } => {
                    let after = stack_after[base as usize..].to_vec();
                    self.call_site(node, *k, base, &image, &after, results)?;
                    image = after;
                }
                Node::Construct(c) if c.kind != Kind::If => {
                    self.require_empty(c)?;
                    self.emit(if c.kind == Kind::Loop {
                        Instruction::Loop(EncBlockType::Empty)
                    } else {
                        Instruction::Block(EncBlockType::Empty)
                    });
                    self.labels.push(true);
                    self.sequence(&c.body, c.before, true, results)?;
                    self.labels.pop();
                    self.emit(Instruction::End);
                }
                Node::Construct(c) => {
                    self.require_empty(c)?;
                    // The condition is the image's top. While rewinding it
                    // becomes whether the call that unwound is in the
                    // then-arm.
                    let condition = self.spill(base + image.len() as u32 - 1, ValType::I32)?;
                    let (lo, hi) = self.arm_range(c);
                    self.state();
                    self.emit(Instruction::I32Const(REWINDING));
                    self.emit(Instruction::I32Eq);
                    self.emit(Instruction::If(EncBlockType::Empty));
                    if lo == hi {
                        self.emit(Instruction::I32Const(0));
                    } else {
                        self.emit(Instruction::LocalGet(self.resume));
                        self.emit(Instruction::I32Const(lo));
                        self.emit(Instruction::I32GeU);
                        self.emit(Instruction::LocalGet(self.resume));
                        self.emit(Instruction::I32Const(hi));
                        self.emit(Instruction::I32LtU);
                        self.emit(Instruction::I32And);
                    }
                    self.emit(Instruction::LocalSet(condition));
                    self.emit(Instruction::End);
                    self.emit(Instruction::LocalGet(condition));
                    self.emit(Instruction::If(EncBlockType::Empty));
                    self.labels.push(true);
                    self.sequence(&c.body, c.before - 1, true, results)?;
                    if let Some(els) = &c.els {
                        self.emit(Instruction::Else);
                        self.sequence(els, c.before - 1, true, results)?;
                    }
                    self.labels.pop();
                    self.emit(Instruction::End);
                    image.pop();
                }
                Node::Op { .. } => unreachable!("only calls and constructs unwind"),
            }
        }
        self.spilled_run(&image, &run, &[], base, !leaves_results)
    }

    /// Reload `image`, run `run`, then keep the stack above `base` (of types
    /// `keep`) in spill locals; skipped while rewinding when `guard`.
    fn spilled_run(
        &mut self,
        image: &[ValType],
        run: &[&Node<'_>],
        keep: &[ValType],
        base: u32,
        guard: bool,
    ) -> Result<(), AsyncifyError> {
        if image.is_empty() && run.is_empty() && keep.is_empty() {
            return Ok(());
        }
        if guard {
            self.state();
            self.emit(Instruction::I32Eqz);
            self.emit(Instruction::If(EncBlockType::Empty));
            self.labels.push(false);
        }
        self.reload(image, base)?;
        for node in run {
            self.verbatim(node)?;
        }
        self.keep(keep, base)?;
        if guard {
            self.labels.pop();
            self.emit(Instruction::End);
        }
        Ok(())
    }

    fn reload(&mut self, image: &[ValType], base: u32) -> Result<(), AsyncifyError> {
        for (n, ty) in image.iter().enumerate() {
            let local = self.spill(base + n as u32, *ty)?;
            self.emit(Instruction::LocalGet(local));
        }
        Ok(())
    }

    fn keep(&mut self, image: &[ValType], base: u32) -> Result<(), AsyncifyError> {
        for (n, ty) in image.iter().enumerate().rev() {
            let local = self.spill(base + n as u32, *ty)?;
            self.emit(Instruction::LocalSet(local));
        }
        Ok(())
    }

    /// A call that can unwind, `k` in program order: it runs normally, or
    /// while rewinding when it is the one that unwound. After it, an unwind
    /// saves the frame and returns.
    fn call_site(
        &mut self,
        node: &Node<'_>,
        k: i32,
        base: u32,
        image: &[ValType],
        after: &[ValType],
        results: &[ValType],
    ) -> Result<(), AsyncifyError> {
        self.state();
        self.emit(Instruction::I32Eqz);
        self.emit(Instruction::LocalGet(self.resume));
        self.emit(Instruction::I32Const(k));
        self.emit(Instruction::I32Eq);
        self.emit(Instruction::I32Or);
        self.emit(Instruction::If(EncBlockType::Empty));
        self.labels.push(false);
        self.reload(image, base)?;
        self.verbatim(node)?;
        self.state();
        self.emit(Instruction::I32Const(UNWINDING));
        self.emit(Instruction::I32Eq);
        self.emit(Instruction::If(EncBlockType::Empty));
        self.emit(Instruction::I32Const(k));
        self.emit(Instruction::LocalSet(self.resume));
        self.unwind(results)?;
        self.emit(Instruction::End);
        self.keep(after, base)?;
        self.labels.pop();
        self.emit(Instruction::End);
        Ok(())
    }

    fn require_empty(&self, c: &Construct<'_>) -> Result<(), AsyncifyError> {
        match c.ty {
            BlockType::Empty => Ok(()),
            BlockType::FuncType(ty) if self.shape.types[ty as usize] == (vec![], vec![]) => Ok(()),
            _ => Err(error(format!(
                "a block that can unwind takes parameters or has results (at {:#x})",
                first_offset(&c.body)
            ))),
        }
    }

    /// The call indices numbered in an `if`'s then-arm, as `[lo, hi)`.
    fn arm_range(&self, c: &Construct<'_>) -> (i32, i32) {
        fn walk(nodes: &[Node<'_>], range: &mut Option<(i32, i32)>) {
            for node in nodes {
                match node {
                    Node::Op { call: Some(k), .. } => {
                        *range = Some(match *range {
                            None => (*k, k + 1),
                            Some((lo, hi)) => (lo.min(*k), hi.max(k + 1)),
                        });
                    }
                    Node::Construct(c) => {
                        walk(&c.body, range);
                        if let Some(els) = &c.els {
                            walk(els, range);
                        }
                    }
                    Node::Op { .. } => {}
                }
            }
        }
        let mut range = None;
        walk(&c.body, &mut range);
        range.unwrap_or((0, 0))
    }

    /// Emit a node as it was, renumbering its branches.
    fn verbatim(&mut self, node: &Node<'_>) -> Result<(), AsyncifyError> {
        match node {
            Node::Op { op, .. } => {
                let instruction = RoundtripReencoder
                    .instruction(op.clone())
                    .map_err(|e| error(format!("{e:?}")))?;
                let instruction = self.relabel(instruction)?;
                self.emit(instruction);
            }
            Node::Construct(c) => {
                let ty = RoundtripReencoder
                    .block_type(c.ty)
                    .map_err(|e| error(format!("{e:?}")))?;
                self.emit(match c.kind {
                    Kind::Block => Instruction::Block(ty),
                    Kind::Loop => Instruction::Loop(ty),
                    Kind::If => Instruction::If(ty),
                });
                self.labels.push(true);
                for node in &c.body {
                    self.verbatim(node)?;
                }
                if let Some(els) = &c.els {
                    self.emit(Instruction::Else);
                    for node in els {
                        self.verbatim(node)?;
                    }
                }
                self.labels.pop();
                self.emit(Instruction::End);
            }
        }
        Ok(())
    }

    fn depth(&self, depth: u32) -> Result<u32, AsyncifyError> {
        let mut originals = 0;
        for (n, original) in self.labels.iter().rev().enumerate() {
            if *original {
                if originals == depth {
                    return Ok(n as u32);
                }
                originals += 1;
            }
        }
        Err(error(format!("branch depth {depth} out of range")))
    }

    fn relabel<'i>(&self, instruction: Instruction<'i>) -> Result<Instruction<'i>, AsyncifyError> {
        Ok(match instruction {
            Instruction::Br(d) => Instruction::Br(self.depth(d)?),
            Instruction::BrIf(d) => Instruction::BrIf(self.depth(d)?),
            Instruction::BrTable(targets, default) => Instruction::BrTable(
                targets
                    .iter()
                    .map(|d| self.depth(*d))
                    .collect::<Result<Vec<_>, _>>()?
                    .into(),
                self.depth(default)?,
            ),
            Instruction::BrOnNull(_)
            | Instruction::BrOnNonNull(_)
            | Instruction::BrOnCast { .. }
            | Instruction::BrOnCastFail { .. }
            | Instruction::Delegate(_)
            | Instruction::Rethrow(_) => {
                return Err(error("unsupported branch"));
            }
            other => other,
        })
    }
}

/// Where a run of nodes starts, for errors.
fn first_offset(nodes: &[Node<'_>]) -> usize {
    match nodes.first() {
        Some(Node::Op { offset, .. }) => *offset,
        Some(Node::Construct(c)) => first_offset(&c.body),
        None => 0,
    }
}

/// Split a sequence at the points where its operand stack is back at `base`.
/// Everything after an unconditional branch is one dead statement.
fn statements<'n, 'a>(nodes: &'n [Node<'a>], base: u32) -> Vec<Statement<'n, 'a>> {
    let mut out = Vec::new();
    let mut start = 0;
    for (n, node) in nodes.iter().enumerate() {
        if node.dead() {
            out.push(Statement {
                nodes: &nodes[start..],
            });
            return out;
        }
        if node.after() == base {
            out.push(Statement {
                nodes: &nodes[start..=n],
            });
            start = n + 1;
        }
    }
    if start < nodes.len() {
        out.push(Statement {
            nodes: &nodes[start..],
        });
    }
    out
}

/// The sequence the next node goes into.
fn current<'b, 'a>(
    stack: &'b mut [Construct<'a>],
    top: &'b mut Vec<Node<'a>>,
) -> &'b mut Vec<Node<'a>> {
    match stack.last_mut() {
        Some(c) => match &mut c.els {
            Some(els) => els,
            None => &mut c.body,
        },
        None => top,
    }
}

/// Read a body into a tree, with operand-stack heights from the validator,
/// numbering the live calls that can unwind in program order.
fn parse<'a>(
    mut validator: wasmparser::FuncValidator<wasmparser::ValidatorResources>,
    body: &FunctionBody<'a>,
    unwinds: &dyn Fn(u32) -> bool,
) -> Result<Vec<Node<'a>>, AsyncifyError> {
    let wrap = |e: wasmparser::BinaryReaderError| error(e.to_string());
    let mut reader = body.get_operators_reader().map_err(wrap)?;
    let mut constructs: Vec<Construct<'a>> = Vec::new();
    let mut top: Vec<Node<'a>> = Vec::new();
    let mut next_call = 0;
    while !reader.eof() {
        let offset = reader.original_position();
        let op = reader.read().map_err(wrap)?;
        // A new frame in dead code starts out reachable; its contents are
        // dead all the same.
        let dead = validator
            .get_control_frame(0)
            .is_some_and(|frame| frame.unreachable)
            || constructs.iter().any(|c| c.dead);
        let before = validator.operand_stack_height();
        let stack = stack_types(&validator, before);
        validator.op(offset, &op).map_err(wrap)?;
        let after = validator.operand_stack_height();
        match op {
            Operator::Block { blockty } | Operator::Loop { blockty } | Operator::If { blockty } => {
                constructs.push(Construct {
                    kind: match op {
                        Operator::Block { .. } => Kind::Block,
                        Operator::Loop { .. } => Kind::Loop,
                        _ => Kind::If,
                    },
                    ty: blockty,
                    body: Vec::new(),
                    els: None,
                    before,
                    stack,
                    after: 0,
                    dead,
                });
            }
            Operator::Else => {
                let c = constructs
                    .last_mut()
                    .ok_or_else(|| error("`else` outside `if`"))?;
                c.els = Some(Vec::new());
            }
            Operator::End => match constructs.pop() {
                Some(mut c) => {
                    c.after = after;
                    current(&mut constructs, &mut top).push(Node::Construct(Box::new(c)));
                }
                None => {
                    if !reader.eof() {
                        return Err(error("operators after the function's end"));
                    }
                }
            },
            Operator::TryTable { .. } | Operator::Try { .. } => {
                return Err(error("exceptions are not supported"));
            }
            op => {
                let call = match op {
                    Operator::Call { function_index } if !dead && unwinds(function_index) => {
                        next_call += 1;
                        Some(next_call - 1)
                    }
                    _ => None,
                };
                let stack_after = stack_types(&validator, after);
                current(&mut constructs, &mut top).push(Node::Op {
                    op,
                    after,
                    stack,
                    stack_after,
                    dead,
                    call,
                    offset,
                });
            }
        }
    }
    Ok(top)
}

/// The operand stack's types, bottom first (unknown ones in dead code as
/// `i32`: dead code is never kept).
fn stack_types(
    validator: &wasmparser::FuncValidator<wasmparser::ValidatorResources>,
    height: u32,
) -> Vec<ValType> {
    (0..height as usize)
        .rev()
        .map(|depth| {
            validator
                .get_operand_type(depth)
                .flatten()
                .unwrap_or(ValType::I32)
        })
        .collect()
}

fn size_of(ty: ValType) -> Result<u32, AsyncifyError> {
    match ty {
        ValType::I32 | ValType::F32 => Ok(4),
        ValType::I64 | ValType::F64 => Ok(8),
        other => Err(error(format!("a local of type {other:?} cannot be saved"))),
    }
}

fn mem32(offset: u64) -> MemArg {
    MemArg {
        offset,
        align: 2,
        memory_index: 0,
    }
}

fn load(ty: ValType, offset: u64) -> Result<Instruction<'static>, AsyncifyError> {
    let arg = |align| MemArg {
        offset,
        align,
        memory_index: 0,
    };
    Ok(match ty {
        ValType::I32 => Instruction::I32Load(arg(2)),
        ValType::I64 => Instruction::I64Load(arg(2)),
        ValType::F32 => Instruction::F32Load(arg(2)),
        ValType::F64 => Instruction::F64Load(arg(2)),
        other => return Err(error(format!("cannot load {other:?}"))),
    })
}

fn store(ty: ValType, offset: u64) -> Result<Instruction<'static>, AsyncifyError> {
    let arg = |align| MemArg {
        offset,
        align,
        memory_index: 0,
    };
    Ok(match ty {
        ValType::I32 => Instruction::I32Store(arg(2)),
        ValType::I64 => Instruction::I64Store(arg(2)),
        ValType::F32 => Instruction::F32Store(arg(2)),
        ValType::F64 => Instruction::F64Store(arg(2)),
        other => return Err(error(format!("cannot store {other:?}"))),
    })
}

fn zero(ty: ValType) -> Result<Instruction<'static>, AsyncifyError> {
    Ok(match ty {
        ValType::I32 => Instruction::I32Const(0),
        ValType::I64 => Instruction::I64Const(0),
        ValType::F32 => Instruction::F32Const(0.0_f32.into()),
        ValType::F64 => Instruction::F64Const(0.0_f64.into()),
        other => return Err(error(format!("cannot return {other:?}"))),
    })
}

#[cfg(test)]
#[path = "asyncify_tests.rs"]
mod tests;
