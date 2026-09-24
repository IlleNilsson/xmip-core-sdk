//! A Rust contract, loadable: the entry point and the contract table
//! `include/xmip_module.h` declares, over any [`ContractFactory`].
//!
//! A provider implements the factory and names it once:
//!
//! ```ignore
//! sdk::export_contract!(
//!     MyFactory, provider = "example", standard = "orders", version = (0, 1, 0)
//! );
//! ```
//!
//! and ships the crate as a `cdylib`. A node opens it, resolves
//! `xmip_create_module_v1` and drives the table; the provider's crate writes
//! no `unsafe` (ADR-0061, decision 8). A module loaded this way is a separate
//! work under whatever license its provider chooses (decision 6).
//!
//! What crosses the boundary: `load` takes the descriptor a Location names —
//! a schema, a pattern, a message type — as UTF-8 and asks the factory for a
//! contract; `validate` reads the whole Stream from the host's reader and
//! answers every [`ValidationIssue`](super::ValidationIssue) as a diagnostic;
//! `implies` answers the `descriptor` the Location bound, as the C module
//! does, and the contract's own `contract`, `version` and `representation`.
//! A contract that cannot judge the bytes at all answers `MALFORMED` with its
//! reason in `last_error`, and a panic is caught and answered as `PANIC`,
//! never unwound into the host.

#![allow(unsafe_code)]

use super::{Contract, ContractFactory};
use core::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use stream::Stream;
use xcore::StreamId;
use xmip_core_abi::XMIP_ABI_VERSION;
use xmip_core_abi::ffi::{
    ContractVtable, Diagnostic, Host, Module, Reader, Str, VtableHeader, WireDescriptor, status,
};

/// The contract table's version this export implements; header section 12.
pub const TRAIT_MAJOR: u32 = 1;
/// See [`TRAIT_MAJOR`].
pub const TRAIT_MINOR: u32 = 0;

/// Who ships the module and its version, as the descriptor carries them.
#[derive(Clone, Copy, Debug)]
pub struct Identity {
    /// The provider's name, the second segment of its repositories' names.
    pub provider: &'static str,
    /// The standard the contract implements: `csv`, `edi-x12`, a provider's
    /// own.
    pub standard: &'static str,
    /// The module's own version.
    pub version: (u32, u32, u32),
}

/// One module instance: the factory, the table the host drives, and what the
/// last call left for the host to borrow.
struct State<F> {
    factory: F,
    table: ContractVtable,
    diagnostics: Vec<Diagnostic>,
    texts: Vec<(String, String)>,
    error: String,
}

/// One contract the factory loaded, as the host holds it, with the
/// descriptor the Location bound it by.
struct Loaded {
    contract: Box<dyn Contract>,
    descriptor: String,
}

/// The entry point's body. [`export_contract!`](crate::export_contract) calls
/// it from the one exported symbol.
///
/// # Safety
/// `host` and `out` must be null or valid for the call, as the header says of
/// `xmip_create_module_v1`. A null pointer or a foreign ABI version is
/// refused, never dereferenced into.
pub unsafe fn create<F: ContractFactory + 'static>(
    host: *const c_void,
    out: *mut c_void,
    factory: F,
    identity: Identity,
) -> i32 {
    let (host, out) = (host.cast::<Host>(), out.cast::<Module>());
    if host.is_null() || out.is_null() {
        return status::INVALID;
    }
    // SAFETY: host is non-null and the caller vouches for it for the call.
    if unsafe { (*host).abi_version } != XMIP_ABI_VERSION {
        return status::UNSUPPORTED;
    }
    let state = Box::into_raw(Box::new(State {
        factory,
        table: table::<F>(),
        diagnostics: Vec::new(),
        texts: Vec::new(),
        error: String::new(),
    }));
    let (major, minor, patch) = identity.version;
    // SAFETY: out is non-null and the caller vouches for it; state was just
    // made, and the table lives in it until `destroy`.
    unsafe {
        *out = Module {
            descriptor: WireDescriptor {
                abi_version: XMIP_ABI_VERSION,
                provider: Str::from_static(identity.provider),
                module: Str::from_static("contract"),
                standard: Str::from_static(identity.standard),
                trait_major: TRAIT_MAJOR,
                trait_minor: TRAIT_MINOR,
                module_major: major,
                module_minor: minor,
                module_patch: patch,
            },
            state: state.cast(),
            vtable: (&raw const (*state).table).cast(),
            last_error: Some(last_error::<F>),
            destroy: Some(destroy::<F>),
        };
    }
    status::OK
}

/// Export a Rust contract as a loadable module: the one symbol a node
/// resolves, `xmip_create_module_v1`, over the factory given, named by its
/// provider, the standard it implements and its own version.
#[macro_export]
macro_rules! export_contract {
    ($factory:expr, provider = $provider:literal, standard = $standard:literal,
     version = ($major:literal, $minor:literal, $patch:literal)) => {
        /// The module's entry point, header section 2.
        ///
        /// # Safety
        /// Called by a host across the C boundary; see
        /// `sdk::contract::export::create`.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn xmip_create_module_v1(
            host: *const ::core::ffi::c_void,
            out: *mut ::core::ffi::c_void,
        ) -> i32 {
            // Evaluated here, outside the unsafe block, so a caller's
            // expression is never run with unsafe permitted.
            let factory = $factory;
            let identity = $crate::contract::export::Identity {
                provider: $provider,
                standard: $standard,
                version: ($major, $minor, $patch),
            };
            // SAFETY: the host's pointers are passed through unchanged.
            unsafe { $crate::contract::export::create(host, out, factory, identity) }
        }
    };
}

const fn table<F: ContractFactory + 'static>() -> ContractVtable {
    ContractVtable {
        header: VtableHeader {
            trait_major: TRAIT_MAJOR,
            trait_minor: TRAIT_MINOR,
            configure: Some(configure),
            start: Some(lifecycle),
            stop: Some(lifecycle),
        },
        load: Some(load::<F>),
        release: Some(release),
        validate: Some(validate::<F>),
        implies: Some(implies),
    }
}

/// What a caller sent as text, or `None` when it is not UTF-8.
///
/// # Safety
/// `text` must describe `len` readable bytes, as the header guarantees for the
/// call.
unsafe fn text_of<'a>(text: Str) -> Option<&'a str> {
    if text.len == 0 {
        return Some("");
    }
    if text.ptr.is_null() {
        return None;
    }
    // SAFETY: the header guarantees ptr..ptr+len for the call.
    std::str::from_utf8(unsafe { core::slice::from_raw_parts(text.ptr, text.len) }).ok()
}

fn str_of(text: &str) -> Str {
    Str {
        ptr: text.as_ptr(),
        len: text.len(),
    }
}

/// Run `call`, answering `PANIC` rather than unwinding into the host.
fn guarded(call: impl FnOnce() -> i32) -> i32 {
    catch_unwind(AssertUnwindSafe(call)).unwrap_or(status::PANIC)
}

unsafe extern "C" fn configure(_state: *mut c_void, _toml: Str) -> i32 {
    status::OK
}

unsafe extern "C" fn lifecycle(_state: *mut c_void) -> i32 {
    status::OK
}

unsafe extern "C" fn load<F: ContractFactory + 'static>(
    state: *mut c_void,
    descriptor: Str,
    out: *mut *mut c_void,
) -> i32 {
    if state.is_null() || out.is_null() {
        return status::INVALID;
    }
    // SAFETY: state is ours, made by `create`; the host serializes calls on
    // one instance.
    let state = unsafe { &mut *state.cast::<State<F>>() };
    // SAFETY: the header guarantees the descriptor's bytes for the call.
    let Some(reference) = (unsafe { text_of(descriptor) }) else {
        state.error = "the descriptor is not UTF-8".to_string();
        return status::INVALID;
    };
    guarded(|| match state.factory.load(reference) {
        Ok(contract) => {
            let descriptor = reference.to_string();
            let loaded = Box::into_raw(Box::new(Loaded {
                contract,
                descriptor,
            }));
            // SAFETY: out is non-null, checked above.
            unsafe { *out = loaded.cast() };
            status::OK
        }
        Err(error) => {
            state.error = error.message;
            status::MALFORMED
        }
    })
}

unsafe extern "C" fn release(_state: *mut c_void, contract: *mut c_void) {
    if !contract.is_null() {
        // SAFETY: only `load` makes these pointers, from Box::into_raw.
        drop(unsafe { Box::from_raw(contract.cast::<Loaded>()) });
    }
}

unsafe extern "C" fn validate<F: ContractFactory + 'static>(
    state: *mut c_void,
    contract: *mut c_void,
    input: *const Reader,
    out: *mut *const Diagnostic,
    out_len: *mut usize,
) -> i32 {
    if state.is_null() || contract.is_null() || input.is_null() || out.is_null() {
        return status::INVALID;
    }
    if out_len.is_null() {
        return status::INVALID;
    }
    // SAFETY: every pointer was checked non-null; state and contract are
    // ours, and the host vouches for the reader for the call.
    let (state, loaded, reader) = unsafe {
        (
            &mut *state.cast::<State<F>>(),
            &*contract.cast::<Loaded>(),
            &*input,
        )
    };
    // SAFETY: out and out_len are non-null; nothing is borrowed until the
    // judgment below says so.
    unsafe {
        *out = core::ptr::null();
        *out_len = 0;
    }
    let bytes = match read_all(reader) {
        Ok(bytes) => bytes,
        Err(code) => return code,
    };
    let stream = Stream::new(StreamId::new(0), bytes, None);
    guarded(|| match loaded.contract.validate(&stream) {
        Ok(result) if result.valid => status::OK,
        Ok(result) => {
            state.texts = result
                .issues
                .into_iter()
                .map(|issue| {
                    let message = format!("{}: {}", issue.code, issue.message);
                    (message, issue.path.unwrap_or_default())
                })
                .collect();
            state.diagnostics = state
                .texts
                .iter()
                .map(|(message, location)| Diagnostic {
                    code: status::CONTRACT,
                    message: str_of(message),
                    location: str_of(location),
                    offset: u64::MAX,
                })
                .collect();
            // SAFETY: as above; the diagnostics live in state until the next
            // call on this instance, as the header says they are borrowed.
            unsafe {
                *out = state.diagnostics.as_ptr();
                *out_len = state.diagnostics.len();
            }
            status::CONTRACT
        }
        Err(error) => {
            state.error = error.message;
            status::MALFORMED
        }
    })
}

/// The whole Stream, read through the host's reader to its end.
fn read_all(reader: &Reader) -> Result<Vec<u8>, i32> {
    let Some(read) = reader.read else {
        return Err(status::INVALID);
    };
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        // SAFETY: chunk is writable for its length; the host vouches for ctx.
        let got = unsafe { read(reader.ctx, chunk.as_mut_ptr(), chunk.len()) };
        if got < 0 {
            return Err(i32::try_from(got).unwrap_or(status::IO));
        }
        if got == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..usize::try_from(got).unwrap_or(0)]);
    }
}

unsafe extern "C" fn implies(
    _state: *mut c_void,
    contract: *mut c_void,
    key: Str,
    out: *mut Str,
) -> i32 {
    if contract.is_null() || out.is_null() {
        return status::INVALID;
    }
    // SAFETY: contract is ours; the header guarantees the key for the call.
    let (loaded, key) = unsafe { (&*contract.cast::<Loaded>(), text_of(key)) };
    let descriptor = loaded.contract.descriptor();
    let answer = match key {
        Some("descriptor") if !loaded.descriptor.is_empty() => &loaded.descriptor,
        Some("contract") => &descriptor.id.0,
        Some("version") => &descriptor.version,
        Some("representation") => &descriptor.representation,
        Some(_) => return status::NOT_FOUND,
        None => return status::INVALID,
    };
    // SAFETY: out is non-null; the answer lives as long as the contract.
    unsafe { *out = str_of(answer) };
    status::OK
}

unsafe extern "C" fn last_error<F: ContractFactory + 'static>(state: *mut c_void) -> Str {
    if state.is_null() {
        return Str::empty();
    }
    // SAFETY: state is ours.
    str_of(&unsafe { &*state.cast::<State<F>>() }.error)
}

unsafe extern "C" fn destroy<F: ContractFactory + 'static>(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: made by Box::into_raw in `create`.
        drop(unsafe { Box::from_raw(state.cast::<State<F>>()) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        ContractDescriptor, ContractError, ContractId, ValidationIssue, ValidationResult,
    };

    /// Text that must be UTF-8, loaded from an empty descriptor only.
    struct Text(ContractDescriptor);

    impl Contract for Text {
        fn descriptor(&self) -> &ContractDescriptor {
            &self.0
        }

        fn identify(&self, _stream: &Stream) -> Result<bool, ContractError> {
            Ok(true)
        }

        fn validate(&self, stream: &Stream) -> Result<ValidationResult, ContractError> {
            if stream.bytes().first() == Some(&0) {
                return Err(ContractError::new("a NUL first is not text of any kind"));
            }
            Ok(ValidationResult::of(
                match std::str::from_utf8(stream.bytes()) {
                    Ok(_) => Vec::new(),
                    Err(error) => vec![ValidationIssue::at(
                        "not-text",
                        "not UTF-8",
                        &format!("byte {}", error.valid_up_to()),
                    )],
                },
            ))
        }
    }

    struct Factory;

    impl ContractFactory for Factory {
        fn technology(&self) -> &'static str {
            "text"
        }

        fn load(&self, reference: &str) -> Result<Box<dyn Contract>, ContractError> {
            if !reference.is_empty() {
                return Err(ContractError::new(format!("text takes no {reference}")));
            }
            Ok(Box::new(Text(ContractDescriptor {
                id: ContractId("text".to_string()),
                version: "1".to_string(),
                representation: "text/plain".to_string(),
            })))
        }
    }

    crate::export_contract!(
        Factory,
        provider = "example",
        standard = "text",
        version = (0, 1, 0)
    );

    struct Source {
        bytes: Vec<u8>,
        at: usize,
    }

    /// Hands the bytes back three at a time, so the export must read to the end.
    unsafe extern "C" fn read_source(ctx: *mut c_void, buf: *mut u8, len: usize) -> i64 {
        // SAFETY: the test owns ctx and the export owns buf for the call.
        let source = unsafe { &mut *ctx.cast::<Source>() };
        let n = (source.bytes.len() - source.at).min(len).min(3);
        // SAFETY: n bytes are readable from the source and writable to buf.
        unsafe { core::ptr::copy_nonoverlapping(source.bytes[source.at..].as_ptr(), buf, n) };
        source.at += n;
        i64::try_from(n).unwrap_or(0)
    }

    fn host(version: u32) -> Host {
        Host {
            abi_version: version,
            ctx: core::ptr::null_mut(),
            log: None,
            cancelled: None,
            journey_id: None,
        }
    }

    fn blank() -> Module {
        Module {
            descriptor: WireDescriptor {
                abi_version: 0,
                provider: Str::empty(),
                module: Str::empty(),
                standard: Str::empty(),
                trait_major: 0,
                trait_minor: 0,
                module_major: 0,
                module_minor: 0,
                module_patch: 0,
            },
            state: core::ptr::null_mut(),
            vtable: core::ptr::null(),
            last_error: None,
            destroy: None,
        }
    }

    fn bytes(text: Str) -> Vec<u8> {
        if text.len == 0 {
            return Vec::new();
        }
        // SAFETY: every Str the export hands back is valid while its source is.
        unsafe { core::slice::from_raw_parts(text.ptr, text.len) }.to_vec()
    }

    type Judged = (i32, Vec<(Vec<u8>, Vec<u8>)>);

    /// Validate `input` through the table: the status, and each diagnostic
    /// as its message and location.
    fn judged(module: &Module, contract: *mut c_void, input: &[u8]) -> Judged {
        // SAFETY: the vtable is the export's contract table.
        let table = unsafe { &*module.vtable.cast::<ContractVtable>() };
        let mut source = Source {
            bytes: input.to_vec(),
            at: 0,
        };
        let reader = Reader {
            ctx: (&raw mut source).cast(),
            read: Some(read_source),
        };
        let mut out: *const Diagnostic = core::ptr::null();
        let mut out_len = 7usize;
        // SAFETY: every pointer is live for the call.
        let verdict = unsafe {
            table.validate.expect("validate")(
                module.state,
                contract,
                &raw const reader,
                &raw mut out,
                &raw mut out_len,
            )
        };
        assert_eq!(
            source.at,
            source.bytes.len(),
            "read to the end in short reads"
        );
        let diagnostics = (0..out_len)
            .map(|index| {
                // SAFETY: out holds out_len diagnostics until the next call.
                let diagnostic = unsafe { &*out.add(index) };
                (bytes(diagnostic.message), bytes(diagnostic.location))
            })
            .collect();
        (verdict, diagnostics)
    }

    #[test]
    fn a_null_or_foreign_host_is_refused_and_out_left_untouched() {
        let mut module = blank();
        let foreign = host(99);
        // SAFETY: both pointers are live.
        let refused =
            unsafe { xmip_create_module_v1((&raw const foreign).cast(), (&raw mut module).cast()) };
        assert_eq!(refused, status::UNSUPPORTED);
        assert!(module.vtable.is_null());
        // SAFETY: a null host is what is being refused.
        let refused = unsafe { xmip_create_module_v1(core::ptr::null(), (&raw mut module).cast()) };
        assert_eq!(refused, status::INVALID);
    }

    #[test]
    fn a_rust_contract_is_driven_through_the_table_as_a_host_drives_it() {
        let mut module = blank();
        let ok = host(XMIP_ABI_VERSION);
        // SAFETY: both pointers are live.
        let made =
            unsafe { xmip_create_module_v1((&raw const ok).cast(), (&raw mut module).cast()) };
        assert_eq!(made, status::OK);
        assert_eq!(bytes(module.descriptor.provider), b"example");
        assert_eq!(bytes(module.descriptor.module), b"contract");
        assert_eq!(bytes(module.descriptor.standard), b"text");
        assert_eq!(module.descriptor.module_minor, 1);

        // SAFETY: the vtable is the export's contract table.
        let table = unsafe { &*module.vtable.cast::<ContractVtable>() };
        let load = table.load.expect("load");
        let mut contract: *mut c_void = core::ptr::null_mut();
        // SAFETY: every pointer is live for the call.
        let refused = unsafe { load(module.state, Str::from_static("x.xsd"), &raw mut contract) };
        assert_eq!(refused, status::MALFORMED);
        // SAFETY: last_error borrows from the state until the next call.
        let why = bytes(unsafe { module.last_error.expect("last_error")(module.state) });
        assert_eq!(why, b"text takes no x.xsd");
        // SAFETY: as above.
        let loaded = unsafe { load(module.state, Str::empty(), &raw mut contract) };
        assert_eq!(loaded, status::OK);

        assert_eq!(
            judged(&module, contract, b"xmip round-trip"),
            (status::OK, Vec::new())
        );
        let (verdict, diagnostics) = judged(&module, contract, &[b'o', b'k', 0xff]);
        assert_eq!(verdict, status::CONTRACT);
        assert_eq!(
            diagnostics,
            vec![(b"not-text: not UTF-8".to_vec(), b"byte 2".to_vec())]
        );
        assert_eq!(judged(&module, contract, &[0, 1]).0, status::MALFORMED);

        let ask = table.implies.expect("implies");
        let mut implied = Str::empty();
        let key = Str::from_static("representation");
        // SAFETY: every pointer is live for the call.
        let answered = unsafe { ask(module.state, contract, key, &raw mut implied) };
        assert_eq!(answered, status::OK);
        assert_eq!(bytes(implied), b"text/plain");
        let key = Str::from_static("delimiters");
        // SAFETY: as above.
        let unknown = unsafe { ask(module.state, contract, key, &raw mut implied) };
        assert_eq!(unknown, status::NOT_FOUND);

        // SAFETY: the contract and the state are the export's, released once.
        unsafe {
            table.release.expect("release")(module.state, contract);
            module.destroy.expect("destroy")(module.state);
        }
    }
}
