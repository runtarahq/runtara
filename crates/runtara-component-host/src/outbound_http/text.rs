//! Deferred strings with an allocation-free decoded length check.
//!
//! WasmStr borrows UTF-8 but allocates when decoding UTF-16/Latin-1. Capture
//! pointer/encoding information during lifting so the request budget is charged
//! before that conversion. All canonical ABI validation remains in WasmStr.
use wasmtime::component::__internal::wasmtime_environ::component::StringEncoding;
use wasmtime::component::__internal::{CanonicalAbiInfo, InstanceType, InterfaceType, LiftContext};
use wasmtime::component::{ComponentType, Lift, WasmStr};

pub(super) struct BorrowedText {
    pub value: WasmStr,
    pub utf8_len: usize,
}

// SAFETY: This type has exactly WasmStr's ABI and delegates its type check and
// lifting. The extra length is host-only metadata and never changes ABI layout.
unsafe impl ComponentType for BorrowedText {
    type Lower = <WasmStr as ComponentType>::Lower;
    const ABI: CanonicalAbiInfo = <WasmStr as ComponentType>::ABI;
    fn typecheck(ty: &InterfaceType, types: &InstanceType<'_>) -> wasmtime::Result<()> {
        <WasmStr as ComponentType>::typecheck(ty, types)
    }
}

// SAFETY: WasmStr validates pointer, alignment, and bounds before we inspect
// memory. Wasmtime 46's string pointer pair is memory32 in both lifting paths.
unsafe impl Lift for BorrowedText {
    fn linear_lift_from_flat(
        cx: &mut LiftContext<'_>,
        ty: InterfaceType,
        src: &Self::Lower,
    ) -> wasmtime::Result<Self> {
        let value = <WasmStr as Lift>::linear_lift_from_flat(cx, ty, src)?;
        Ok(Self {
            value,
            utf8_len: decoded_len(cx, src[0].get_u32() as usize, src[1].get_u32() as usize),
        })
    }
    fn linear_lift_from_memory(
        cx: &mut LiftContext<'_>,
        ty: InterfaceType,
        bytes: &[u8],
    ) -> wasmtime::Result<Self> {
        let value = <WasmStr as Lift>::linear_lift_from_memory(cx, ty, bytes)?;
        let ptr = u32::from_le_bytes(bytes[..4].try_into()?) as usize;
        let len = u32::from_le_bytes(bytes[4..8].try_into()?) as usize;
        Ok(Self {
            value,
            utf8_len: decoded_len(cx, ptr, len),
        })
    }
}

fn decoded_len(cx: &LiftContext<'_>, ptr: usize, len: usize) -> usize {
    let memory = cx.memory();
    match cx.options().string_encoding {
        StringEncoding::Utf8 => len,
        StringEncoding::Utf16 => utf16_len(&memory[ptr..ptr + len * 2]),
        StringEncoding::CompactUtf16 if len & (1 << 31) != 0 => {
            utf16_len(&memory[ptr..ptr + (len ^ (1 << 31)) * 2])
        }
        StringEncoding::CompactUtf16 => memory[ptr..ptr + len]
            .iter()
            .map(|byte| if *byte < 128 { 1 } else { 2 })
            .try_fold(0usize, bounded_add)
            .unwrap_or(usize::MAX),
    }
}

fn bounded_add(total: usize, size: usize) -> Option<usize> {
    let total = total.checked_add(size)?;
    (total <= super::MAX_REQUEST_BYTES).then_some(total)
}

fn utf16_len(bytes: &[u8]) -> usize {
    char::decode_utf16(
        bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]])),
    )
    .try_fold(0usize, |total, character| {
        bounded_add(total, character.ok()?.len_utf8())
    })
    .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf16_budget_uses_decoded_bytes_without_allocating_the_string() {
        let bytes: Vec<_> = "aé😀".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(utf16_len(&bytes), "aé😀".len());
        assert_eq!(utf16_len(&0xd800u16.to_le_bytes()), usize::MAX);
        assert_eq!(bounded_add(super::super::MAX_REQUEST_BYTES, 1), None);
    }
    #[test]
    fn canonical_string_encodings_are_measured_before_conversion() -> anyhow::Result<()> {
        use wasmtime::component::{Component, Linker};
        for (encoding, bytes, units, expected) in [
            ("utf8", "é😀".as_bytes().to_vec(), 6, "é😀"),
            (
                "utf16",
                "é😀".encode_utf16().flat_map(u16::to_le_bytes).collect(),
                3,
                "é😀",
            ),
            ("latin1+utf16", vec![0xe9], 1, "é"),
        ] {
            let data = bytes
                .iter()
                .map(|byte| format!("\\{byte:02x}"))
                .collect::<String>();
            let source = format!(
                r#"(component
                (import "inspect" (func $inspect (param "text" string) (result u32)))
                (core module $memory (memory (export "memory") 1) (data (i32.const 16) "{data}"))
                (core instance $memory (instantiate $memory))
                (core func $inspect (canon lower (func $inspect) (memory $memory "memory") string-encoding={encoding}))
                (core module $main
                    (import "host" "inspect" (func $inspect (param i32 i32) (result i32)))
                    (func (export "run") (result i32) i32.const 16 i32.const {units} call $inspect))
                (core instance $main (instantiate $main (with "host" (instance (export "inspect" (func $inspect))))))
                (func (export "run") (result u32) (canon lift (core func $main "run"))))"#
            );
            let mut config = wasmtime::Config::new();
            config.wasm_component_model(true);
            let engine = wasmtime::Engine::new(&config)?;
            let component = Component::new(&engine, wat::parse_str(source)?)?;
            let mut linker = Linker::<()>::new(&engine);
            linker
                .root()
                .func_wrap("inspect", move |store, (text,): (BorrowedText,)| {
                    assert_eq!(text.utf8_len, expected.len());
                    assert_eq!(text.value.to_str(&store)?, expected);
                    Ok((text.utf8_len as u32,))
                })?;
            let mut store = wasmtime::Store::new(&engine, ());
            let instance = linker.instantiate(&mut store, &component)?;
            let run = instance.get_typed_func::<(), (u32,)>(&mut store, "run")?;
            assert_eq!(run.call(&mut store, ())?.0 as usize, expected.len());
        }
        Ok(())
    }
}
