//! IOKit / CoreFoundation FFI for the macOS sensors (see the parent module).
//!
//! Hand-written declarations for the handful of functions used, so no extra
//! crate is needed. The HID event system functions are not in Apple's public
//! headers, but they are exported by IOKit and have been stable for years.
//! Every failure degrades to "no readings" instead of an error.

use std::ffi::{c_char, c_void, CStr, CString};
use std::mem::size_of;
use std::ptr;

use super::{
    decode_smc, fourcc, hid_wanted, parse_fan_name, FanReading, HidTemp, MacBackend, MacSample, SmcKeyData,
    SmcTempReading, SMC_CMD_READ_BYTES, SMC_CMD_READ_KEYINFO, SMC_SELECTOR, SMC_TEMPS,
};
use crate::sensors::Wanted;

type CFTypeRef = *const c_void;
type CFIndex = isize;

const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8
const CF_NUMBER_SINT32: CFIndex = 3; // kCFNumberSInt32Type

/// `kIOHIDEventTypeTemperature`; its float field is `type << 16`.
const HID_EVENT_TEMPERATURE: i64 = 15;
const HID_FIELD_TEMPERATURE_LEVEL: i32 = 15 << 16;

#[repr(C)]
struct Opaque {
    _p: [u8; 0],
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: Opaque;
    static kCFTypeDictionaryValueCallBacks: Opaque;
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFTypeRef;
    fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: CFIndex, encoding: u32) -> u8;
    fn CFNumberCreate(alloc: CFTypeRef, kind: CFIndex, value: *const c_void) -> CFTypeRef;
    fn CFDictionaryCreate(
        alloc: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        count: CFIndex,
        key_callbacks: *const Opaque,
        value_callbacks: *const Opaque,
    ) -> CFTypeRef;
    fn CFArrayGetCount(array: CFTypeRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(array: CFTypeRef, index: CFIndex) -> CFTypeRef;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    // HID event system (Apple Silicon temperature sensors).
    fn IOHIDEventSystemClientCreate(alloc: CFTypeRef) -> CFTypeRef;
    fn IOHIDEventSystemClientSetMatching(client: CFTypeRef, matching: CFTypeRef) -> i32;
    fn IOHIDEventSystemClientCopyServices(client: CFTypeRef) -> CFTypeRef;
    fn IOHIDServiceClientCopyProperty(service: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn IOHIDServiceClientCopyEvent(service: CFTypeRef, kind: i64, options: i32, timestamp: i64) -> CFTypeRef;
    fn IOHIDEventGetFloatValue(event: CFTypeRef, field: i32) -> f64;

    // SMC user client.
    fn IOServiceMatching(name: *const c_char) -> CFTypeRef;
    fn IOServiceGetMatchingService(main_port: u32, matching: CFTypeRef) -> u32;
    fn IOServiceOpen(service: u32, owning_task: u32, kind: u32, connect: *mut u32) -> i32;
    fn IOServiceClose(connect: u32) -> i32;
    fn IOObjectRelease(object: u32) -> i32;
    fn IOConnectCallStructMethod(
        connection: u32,
        selector: u32,
        input: *const c_void,
        input_size: usize,
        output: *mut c_void,
        output_size: *mut usize,
    ) -> i32;
}

extern "C" {
    /// `mach_task_self()` is a macro over this variable.
    static mach_task_self_: u32;
}

/// Owned CoreFoundation object, released on drop.
struct Cf(CFTypeRef);

impl Cf {
    fn string(s: &str) -> Option<Self> {
        let c = CString::new(s).ok()?;
        // SAFETY: `c` is a valid NUL-terminated string; a null allocator is the default one.
        let r = unsafe { CFStringCreateWithCString(ptr::null(), c.as_ptr(), UTF8) };
        (!r.is_null()).then_some(Self(r))
    }

    fn number(v: i32) -> Option<Self> {
        // SAFETY: `v` outlives the call, which copies it.
        let r = unsafe { CFNumberCreate(ptr::null(), CF_NUMBER_SINT32, &v as *const i32 as *const c_void) };
        (!r.is_null()).then_some(Self(r))
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: `self.0` is an owned, non-null CF object.
        unsafe { CFRelease(self.0) }
    }
}

/// UTF-8 contents of a CFString, if `s` is one.
///
/// # Safety
/// `s` must be null or a valid CF object.
unsafe fn cf_to_string(s: CFTypeRef) -> Option<String> {
    if s.is_null() || CFGetTypeID(s) != CFStringGetTypeID() {
        return None;
    }
    let mut buf = [0 as c_char; 256];
    if CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as CFIndex, UTF8) == 0 {
        return None;
    }
    Some(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
}

// -- HID temperature sensors --------------------------------------------------------------

struct Hid {
    client: Cf,
    /// Keeps the service references below alive.
    services: Cf,
    named: Vec<(CFTypeRef, String)>,
}

impl Hid {
    fn open() -> Option<Self> {
        // SAFETY: plain calls into IOKit/CoreFoundation; every returned
        // reference is null-checked and owned by a `Cf` that releases it.
        unsafe {
            let client = IOHIDEventSystemClientCreate(ptr::null());
            if client.is_null() {
                return None;
            }
            let client = Cf(client);

            // PrimaryUsagePage 0xff00 (Apple vendor), PrimaryUsage 5: temperature sensors.
            let keys = [Cf::string("PrimaryUsagePage")?, Cf::string("PrimaryUsage")?];
            let values = [Cf::number(0xff00)?, Cf::number(5)?];
            let k = [keys[0].0, keys[1].0];
            let v = [values[0].0, values[1].0];
            let dict = CFDictionaryCreate(
                ptr::null(),
                k.as_ptr(),
                v.as_ptr(),
                2,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            if dict.is_null() {
                return None;
            }
            let dict = Cf(dict);
            IOHIDEventSystemClientSetMatching(client.0, dict.0);

            let services = IOHIDEventSystemClientCopyServices(client.0);
            if services.is_null() {
                return None;
            }
            let services = Cf(services);

            let product = Cf::string("Product")?;
            let mut named = Vec::new();
            for i in 0..CFArrayGetCount(services.0) {
                let svc = CFArrayGetValueAtIndex(services.0, i);
                if svc.is_null() {
                    continue;
                }
                let prop = IOHIDServiceClientCopyProperty(svc, product.0);
                let name = cf_to_string(prop);
                if !prop.is_null() {
                    CFRelease(prop);
                }
                if let Some(name) = name {
                    named.push((svc, name));
                }
            }
            (!named.is_empty()).then_some(Self { client, services, named })
        }
    }

    fn sample(&self, wanted: Option<&Wanted>) -> Vec<HidTemp> {
        let _ = (&self.client, &self.services); // alive for as long as `named`
        let mut out = Vec::new();
        for (svc, name) in &self.named {
            if wanted.is_some_and(|w| !hid_wanted(name, w)) {
                continue;
            }
            // SAFETY: `svc` is kept alive by `self.services`; the event is
            // released right after reading it.
            unsafe {
                let ev = IOHIDServiceClientCopyEvent(*svc, HID_EVENT_TEMPERATURE, 0, 0);
                if ev.is_null() {
                    continue;
                }
                let value = IOHIDEventGetFloatValue(ev, HID_FIELD_TEMPERATURE_LEVEL);
                CFRelease(ev);
                out.push(HidTemp { product: name.clone(), value: value as f32 });
            }
        }
        out
    }
}

// -- SMC ---------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct KeyInfo {
    size: u32,
    data_type: u32,
}

struct FanKeys {
    index: u32,
    ac: u32,
    info: KeyInfo,
    name: Option<String>,
}

struct Smc {
    conn: u32,
    temps: Vec<(&'static str, u32, KeyInfo)>,
    fans: Vec<FanKeys>,
}

impl Smc {
    fn open() -> Option<Self> {
        // SAFETY: standard IOKit service lookup; the service is released after
        // opening, and the connection in `Drop`.
        let conn = unsafe {
            let service = IOServiceGetMatchingService(0, IOServiceMatching(b"AppleSMC\0".as_ptr() as *const c_char));
            if service == 0 {
                return None;
            }
            let mut conn = 0u32;
            let kr = IOServiceOpen(service, mach_task_self_, 0, &mut conn);
            IOObjectRelease(service);
            if kr != 0 || conn == 0 {
                return None;
            }
            conn
        };
        let mut smc = Self { conn, temps: Vec::new(), fans: Vec::new() };
        smc.probe();
        Some(smc)
    }

    /// Calls the SMC with `input`; `None` when the call or the SMC reports an error
    /// (an unknown key answers with result 0x84).
    fn call(&self, input: &SmcKeyData) -> Option<SmcKeyData> {
        let mut out = SmcKeyData::default();
        let mut out_size = size_of::<SmcKeyData>();
        // SAFETY: both structs are `repr(C)` with the 80-byte layout of the
        // driver's `SMCKeyData_t`, and the sizes passed are theirs.
        let kr = unsafe {
            IOConnectCallStructMethod(
                self.conn,
                SMC_SELECTOR,
                input as *const SmcKeyData as *const c_void,
                size_of::<SmcKeyData>(),
                &mut out as *mut SmcKeyData as *mut c_void,
                &mut out_size,
            )
        };
        (kr == 0 && out.result == 0).then_some(out)
    }

    fn key_info(&self, key: u32) -> Option<KeyInfo> {
        let input = SmcKeyData { key, data8: SMC_CMD_READ_KEYINFO, ..Default::default() };
        let out = self.call(&input)?;
        let (size, data_type) = (out.key_info.data_size, out.key_info.data_type);
        (size > 0 && size <= 32).then_some(KeyInfo { size, data_type })
    }

    fn read_bytes(&self, key: u32, info: KeyInfo) -> Option<Vec<u8>> {
        let mut input = SmcKeyData { key, data8: SMC_CMD_READ_BYTES, ..Default::default() };
        input.key_info.data_size = info.size;
        let out = self.call(&input)?;
        Some(out.bytes[..info.size as usize].to_vec())
    }

    fn read_value(&self, key: u32, info: KeyInfo) -> Option<f32> {
        decode_smc(info.data_type, &self.read_bytes(key, info)?)
    }

    /// Finds which of the known keys this Mac has, and its fans.
    fn probe(&mut self) {
        for def in SMC_TEMPS {
            let key = fourcc(def.key);
            if let Some(info) = self.key_info(key) {
                self.temps.push((def.key, key, info));
            }
        }
        let count = self
            .key_info(fourcc("FNum"))
            .and_then(|info| self.read_value(fourcc("FNum"), info))
            .map_or(0, |n| n.clamp(0.0, 16.0) as u32);
        for index in 0..count {
            let ac = fourcc(&format!("F{index}Ac"));
            let Some(info) = self.key_info(ac) else { continue };
            let id = fourcc(&format!("F{index}ID"));
            let name = self
                .key_info(id)
                .and_then(|i| self.read_bytes(id, i))
                .and_then(|b| parse_fan_name(&b));
            self.fans.push(FanKeys { index, ac, info, name });
        }
    }

    fn sample(&self) -> (Vec<SmcTempReading>, Vec<FanReading>) {
        let temps = self
            .temps
            .iter()
            .filter_map(|&(name, key, info)| {
                Some(SmcTempReading { key: name.into(), value: self.read_value(key, info)? })
            })
            .collect();
        let fans = self
            .fans
            .iter()
            .filter_map(|f| {
                Some(FanReading { index: f.index, name: f.name.clone(), rpm: self.read_value(f.ac, f.info)? })
            })
            .collect();
        (temps, fans)
    }
}

impl Drop for Smc {
    fn drop(&mut self) {
        // SAFETY: `conn` was opened by `IOServiceOpen` and is closed once.
        unsafe {
            IOServiceClose(self.conn);
        }
    }
}

// -- the backend --------------------------------------------------------------------------------

/// Reads both sources; either may be missing (HID on old Intel Macs, the SMC
/// when the user client cannot be opened).
pub struct IoKitBackend {
    hid: Option<Hid>,
    smc: Option<Smc>,
}

// SAFETY: the CF/IOKit references are only used through `&mut self` from one
// thread at a time; IOKit objects may be used from any thread.
unsafe impl Send for IoKitBackend {}

impl IoKitBackend {
    pub fn open() -> Self {
        Self { hid: Hid::open(), smc: Smc::open() }
    }
}

impl MacBackend for IoKitBackend {
    fn sample(&mut self, wanted: Option<&Wanted>) -> MacSample {
        let hid = self.hid.as_ref().map(|h| h.sample(wanted)).unwrap_or_default();
        let (smc_temps, fans) = self.smc.as_ref().map(Smc::sample).unwrap_or_default();
        MacSample { hid, smc_temps, fans }
    }
}
