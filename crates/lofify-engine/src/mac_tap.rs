//! System audio capture on macOS 14.2+ through a Core Audio **process tap**. No virtual driver.
//!
//! **UNTESTED on real hardware.** Written against the objc2-core-audio 0.3 bindings and
//! compile-checked with `cargo check --target aarch64-apple-darwin`.
//!
//! The sequence follows Apple's "Capturing system audio with Core Audio taps" and the brief:
//!
//! 1. The caller has already opened the output stream, so this process has a Core Audio process
//!    object. Translate our PID to that object (`kAudioHardwarePropertyTranslatePIDToProcessObject`).
//! 2. Create a **global stereo tap that excludes our own process**
//!    (`CATapDescription::initStereoGlobalTapButExcludeProcesses`) so Lofify never records its own
//!    output, with mute behaviour `MutedWhenTapped` so the original audio is silenced while the
//!    tap runs. cpal's own loopback code hardcodes `Unmuted`, which is why this is not cpal's.
//! 3. Create a **private aggregate device** containing the tap (drift compensation on) whose main
//!    sub-device is the default output device, so it is clocked by the device that plays the audio.
//! 4. Attach an IOProc to the aggregate device and push each buffer into the capture ring.
//!
//! Dropping [`MacTap`] stops the IOProc and destroys the aggregate device and the tap. The tap is
//! process scoped, so if Lofify crashes macOS destroys it and the original audio comes back.
//!
//! Needs `NSAudioCaptureUsageDescription` in the app's Info.plist (a bare `cargo run` binary
//! inherits the permission of the terminal it runs from).

use crate::backend::CaptureSource;
use crate::capture::CaptureWriter;
use crate::error::{EngineError, Result};
use crate::types::{SYSTEM_AUDIO_ID, SYSTEM_AUDIO_NAME, StreamInfo};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart,
    AudioDeviceStop, AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap,
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress, CATapDescription,
    CATapMuteBehavior, kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceMainSubDeviceKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceSubDeviceListKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey, kAudioDevicePropertyDeviceUID,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyTranslatePIDToProcessObject,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
    kAudioSubDeviceUIDKey, kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey,
    kAudioTapPropertyFormat,
};
use objc2_core_audio_types::{
    AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp, kAudioFormatFlagIsFloat,
    kAudioFormatFlagIsNonInterleaved,
};
use objc2_core_foundation::{
    CFArray, CFDictionary, CFMutableDictionary, CFRetained, CFString, kCFAllocatorDefault,
    kCFTypeArrayCallBacks, kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks,
};
use objc2_foundation::{NSArray, NSNumber, NSProcessInfo, NSString};
use std::ffi::{CStr, c_void};
use std::mem::MaybeUninit;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// `AudioObjectID` value meaning "no such object".
const UNKNOWN_OBJECT: AudioObjectID = 0;
/// Frames the de-interleaving scratch buffer can hold; larger callbacks are processed in pieces.
const SCRATCH_FRAMES: usize = 8192;
/// UTF-8 encoding constant for `CFStringCreateWithCString`.
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

static INSTANCE: AtomicU32 = AtomicU32::new(0);

/// True when this Mac can create process taps (macOS 14.2 or newer).
pub fn supported() -> bool {
    let v = NSProcessInfo::processInfo().operatingSystemVersion();
    (v.majorVersion, v.minorVersion) >= (14, 2)
}

/// "14.1.2" style version string for error messages.
pub fn os_version() -> String {
    let v = NSProcessInfo::processInfo().operatingSystemVersion();
    format!("{}.{}.{}", v.majorVersion, v.minorVersion, v.patchVersion)
}

fn backend_error(what: &str, status: i32) -> EngineError {
    EngineError::Backend(format!("{what} failed (Core Audio status {status})"))
}

fn check(status: i32, what: &str) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(backend_error(what, status))
    }
}

/// Reads one fixed-size property. `qualifier` is passed through for the few properties that
/// need one (the PID translation).
///
/// # Safety
/// `T` must be the exact type Core Audio writes for `selector`.
unsafe fn get_property<T>(
    object: AudioObjectID,
    selector: u32,
    qualifier: Option<&[u8]>,
) -> std::result::Result<T, i32> {
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut size = std::mem::size_of::<T>() as u32;
    let mut out = MaybeUninit::<T>::uninit();
    let (q_size, q_ptr) = match qualifier {
        Some(q) => (q.len() as u32, q.as_ptr().cast::<c_void>()),
        None => (0, std::ptr::null()),
    };
    // SAFETY: all pointers are valid for the duration of the call and `size` matches `T`.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            q_size,
            q_ptr,
            NonNull::from(&mut size),
            NonNull::new_unchecked(out.as_mut_ptr().cast::<c_void>()),
        )
    };
    if status == 0 {
        // SAFETY: Core Audio reported success, so it initialised `out`.
        Ok(unsafe { out.assume_init() })
    } else {
        Err(status)
    }
}

/// Our own process's Core Audio object. It only exists once we are playing audio, and may take a
/// moment to appear after the output stream starts, so retry briefly.
fn own_process_object() -> Result<AudioObjectID> {
    let pid = std::process::id() as i32;
    for _ in 0..40 {
        // SAFETY: the translation property returns one AudioObjectID for a pid_t qualifier.
        let object = unsafe {
            get_property::<AudioObjectID>(
                kAudioObjectSystemObject as AudioObjectID,
                kAudioHardwarePropertyTranslatePIDToProcessObject,
                Some(&pid.to_ne_bytes()),
            )
        };
        match object {
            Ok(id) if id != UNKNOWN_OBJECT => return Ok(id),
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    Err(EngineError::Backend(
        "Lofify has no Core Audio process object yet (the output stream must be playing before the system tap can exclude it)"
            .into(),
    ))
}

/// The UID of the current default output device, used as the aggregate device's clock.
fn default_output_uid() -> Result<Retained<NSString>> {
    // SAFETY: both properties return a single AudioObjectID / CFStringRef respectively.
    unsafe {
        let device = get_property::<AudioObjectID>(
            kAudioObjectSystemObject as AudioObjectID,
            kAudioHardwarePropertyDefaultOutputDevice,
            None,
        )
        .map_err(|s| backend_error("reading the default output device", s))?;
        if device == UNKNOWN_OBJECT {
            return Err(EngineError::NoDefaultOutput);
        }
        let raw = get_property::<*mut NSString>(device, kAudioDevicePropertyDeviceUID, None)
            .map_err(|s| backend_error("reading the default output device UID", s))?;
        // The property follows the "Get" rule of +1 ownership, so take it over without retaining.
        Retained::from_raw(raw).ok_or(EngineError::NoDefaultOutput)
    }
}

fn cf_key(key: &'static CStr) -> Result<CFRetained<CFString>> {
    // SAFETY: `key` is a valid NUL-terminated string that outlives the call.
    unsafe { CFString::with_c_string(kCFAllocatorDefault, key.as_ptr(), K_CF_STRING_ENCODING_UTF8) }
        .ok_or_else(|| EngineError::Backend("could not create a Core Foundation string".into()))
}

fn dict(capacity: isize) -> Result<CFRetained<CFMutableDictionary>> {
    // SAFETY: the standard CFType callbacks are valid for the life of the program.
    unsafe {
        CFMutableDictionary::new(
            kCFAllocatorDefault,
            capacity,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    }
    .ok_or_else(|| EngineError::Backend("could not create a Core Foundation dictionary".into()))
}

fn set(d: &CFMutableDictionary, key: &'static CStr, value: *const c_void) -> Result<()> {
    let key = cf_key(key)?;
    // SAFETY: key and value are valid CF objects retained by the dictionary's callbacks.
    unsafe {
        CFMutableDictionary::set_value(Some(d), &*key as *const CFString as *const c_void, value)
    };
    Ok(())
}

fn cf_array(items: &[*const c_void]) -> Result<CFRetained<CFArray>> {
    // SAFETY: `items` are valid CF objects; the array copies and retains them.
    unsafe {
        CFArray::new(
            kCFAllocatorDefault,
            items.as_ptr() as *mut *const c_void,
            items.len() as isize,
            &kCFTypeArrayCallBacks,
        )
    }
    .ok_or_else(|| EngineError::Backend("could not create a Core Foundation array".into()))
}

/// Properties of the private aggregate device: the tap plus the default output as clock.
fn aggregate_properties(
    tap_uid: &NSString,
    output_uid: &NSString,
    name: &str,
    uid: &str,
) -> Result<CFRetained<CFDictionary>> {
    let yes = NSNumber::new_bool(true);
    let no = NSNumber::new_bool(false);
    let as_ptr = |n: &NSNumber| n as *const NSNumber as *const c_void;

    let tap = dict(2)?;
    set(
        &tap,
        kAudioSubTapUIDKey,
        tap_uid as *const NSString as *const c_void,
    )?;
    set(&tap, kAudioSubTapDriftCompensationKey, as_ptr(&yes))?;
    let taps = cf_array(&[&*tap as *const CFMutableDictionary as *const c_void])?;

    let sub = dict(1)?;
    set(
        &sub,
        kAudioSubDeviceUIDKey,
        output_uid as *const NSString as *const c_void,
    )?;
    let subs = cf_array(&[&*sub as *const CFMutableDictionary as *const c_void])?;

    let agg = dict(8)?;
    let name = CFString::from_str(name);
    let uid = CFString::from_str(uid);
    set(
        &agg,
        kAudioAggregateDeviceNameKey,
        &*name as *const CFString as *const c_void,
    )?;
    set(
        &agg,
        kAudioAggregateDeviceUIDKey,
        &*uid as *const CFString as *const c_void,
    )?;
    set(
        &agg,
        kAudioAggregateDeviceMainSubDeviceKey,
        output_uid as *const NSString as *const c_void,
    )?;
    set(
        &agg,
        kAudioAggregateDeviceSubDeviceListKey,
        &*subs as *const CFArray as *const c_void,
    )?;
    set(
        &agg,
        kAudioAggregateDeviceTapListKey,
        &*taps as *const CFArray as *const c_void,
    )?;
    set(&agg, kAudioAggregateDeviceTapAutoStartKey, as_ptr(&yes))?;
    set(&agg, kAudioAggregateDeviceIsPrivateKey, as_ptr(&yes))?;
    set(&agg, kAudioAggregateDeviceIsStackedKey, as_ptr(&no))?;
    // SAFETY: a CFMutableDictionary is a CFDictionary.
    Ok(unsafe { CFRetained::cast_unchecked::<CFDictionary>(agg) })
}

/// State the IOProc needs. Owned by [`MacTap`], freed only after the IOProc is destroyed.
struct CallbackContext {
    writer: CaptureWriter,
    channels: usize,
    interleaved: bool,
    scratch: Vec<f32>,
}

impl CallbackContext {
    /// Pushes the tap's audio from one input buffer list into the ring.
    ///
    /// # Safety
    /// `list` must be the valid buffer list Core Audio passed to the IOProc.
    unsafe fn handle(&mut self, list: &AudioBufferList) {
        let count = list.mNumberBuffers as usize;
        // SAFETY: `mBuffers` is a C flexible array of `mNumberBuffers` entries.
        let buffers = unsafe { std::slice::from_raw_parts(list.mBuffers.as_ptr(), count) };
        if self.interleaved {
            // The tap is the last input stream of the aggregate device (sub-device inputs, if
            // any, come first), so take the last buffer with the tap's channel count.
            let Some(buf) = buffers
                .iter()
                .rev()
                .find(|b| b.mNumberChannels as usize == self.channels)
            else {
                return;
            };
            if buf.mData.is_null() {
                return;
            }
            let samples = buf.mDataByteSize as usize / std::mem::size_of::<f32>();
            // SAFETY: Core Audio guarantees `mData` holds `mDataByteSize` bytes of f32 samples.
            let data = unsafe { std::slice::from_raw_parts(buf.mData as *const f32, samples) };
            self.writer.write(data, self.channels);
        } else {
            // One mono buffer per channel: take the last `channels` buffers and interleave.
            if count < self.channels {
                return;
            }
            let planes = &buffers[count - self.channels..];
            let frames = planes
                .iter()
                .map(|b| b.mDataByteSize as usize / std::mem::size_of::<f32>())
                .min()
                .unwrap_or(0);
            let mut done = 0;
            while done < frames {
                let n = (frames - done).min(SCRATCH_FRAMES);
                for (ch, plane) in planes.iter().enumerate() {
                    if plane.mData.is_null() {
                        return;
                    }
                    // SAFETY: as above, each plane holds `frames` f32 samples.
                    let data =
                        unsafe { std::slice::from_raw_parts(plane.mData as *const f32, frames) };
                    for i in 0..n {
                        self.scratch[i * self.channels + ch] = data[done + i];
                    }
                }
                self.writer
                    .write(&self.scratch[..n * self.channels], self.channels);
                done += n;
            }
        }
    }
}

/// The IOProc: runs on a Core Audio real-time thread, so it must never panic or unwind.
unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `client` is the context pointer registered in `MacTap::open`, which stays valid
        // until the IOProc has been stopped and destroyed; `input` is valid for this call.
        unsafe {
            if let Some(ctx) = (client as *mut CallbackContext).as_mut() {
                ctx.handle(input.as_ref());
            }
        }
    }));
    0
}

/// A running system audio tap. Dropping it stops the capture and removes the tap.
pub struct MacTap {
    aggregate: AudioObjectID,
    tap: AudioObjectID,
    io_proc: AudioDeviceIOProcID,
    context: *mut CallbackContext,
    info: StreamInfo,
}

impl std::fmt::Debug for MacTap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MacTap")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl CaptureSource for MacTap {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
}

impl MacTap {
    /// Creates the tap and starts capturing into `writer`. Call only after the output stream is
    /// playing (see the module docs).
    pub fn open(writer: CaptureWriter) -> Result<MacTap> {
        if !supported() {
            return Err(EngineError::Unsupported(format!(
                "system audio capture needs macOS 14.2 or newer (this Mac runs {})",
                os_version()
            )));
        }
        let process = own_process_object()?;
        let instance = INSTANCE.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();

        // 1. Global stereo tap excluding ourselves, original audio muted while tapped.
        let excluded = NSArray::from_retained_slice(&[NSNumber::new_u32(process)]);
        // SAFETY: standard Objective-C allocation and initialisation of a CATapDescription.
        let description = unsafe {
            CATapDescription::initStereoGlobalTapButExcludeProcesses(
                CATapDescription::alloc(),
                &excluded,
            )
        };
        // SAFETY: plain property setters on a live object.
        unsafe {
            description.setMuteBehavior(CATapMuteBehavior::MutedWhenTapped);
            description.setName(&NSString::from_str(&format!(
                "Lofify system audio {pid}.{instance}"
            )));
            description.setPrivate(true);
        }
        let mut tap: AudioObjectID = UNKNOWN_OBJECT;
        // SAFETY: `tap` is a valid out pointer and the description is alive.
        let status = unsafe { AudioHardwareCreateProcessTap(Some(&description), &mut tap) };
        if status != 0 {
            return Err(EngineError::PermissionDenied(format!(
                "macOS refused to create the system audio tap (Core Audio status {status}). \
                 Allow Lofify under System Settings > Privacy & Security > Screen & System Audio Recording \
                 (System Audio Recording Only), then try again"
            )));
        }
        let destroy_tap = || {
            // SAFETY: `tap` was created above.
            let _ = unsafe { AudioHardwareDestroyProcessTap(tap) };
        };

        // 2. The tap's format tells us the rate and channel layout.
        // SAFETY: kAudioTapPropertyFormat returns one AudioStreamBasicDescription.
        let format = match unsafe {
            get_property::<AudioStreamBasicDescription>(tap, kAudioTapPropertyFormat, None)
        } {
            Ok(f) => f,
            Err(s) => {
                destroy_tap();
                return Err(backend_error("reading the tap format", s));
            }
        };
        if format.mFormatFlags & kAudioFormatFlagIsFloat == 0 || format.mBitsPerChannel != 32 {
            destroy_tap();
            return Err(EngineError::UnsupportedFormat(format!(
                "the system tap delivers {} bit samples (flags {:#x}); 32 bit float is required",
                format.mBitsPerChannel, format.mFormatFlags
            )));
        }
        let channels = (format.mChannelsPerFrame as usize).max(1);
        let interleaved = format.mFormatFlags & kAudioFormatFlagIsNonInterleaved == 0;
        let sample_rate = format.mSampleRate.round() as u32;

        // 3. Private aggregate device: the tap, clocked by the default output device.
        let built = (|| -> Result<AudioObjectID> {
            let output_uid = default_output_uid()?;
            // SAFETY: UUID() returns the tap's NSUUID; UUIDString() an NSString.
            let tap_uid = unsafe { description.UUID().UUIDString() };
            let props = aggregate_properties(
                &tap_uid,
                &output_uid,
                &format!("Lofify system audio {pid}.{instance}"),
                &format!("io.github.lofify.SystemAudioTap.{pid}.{instance}"),
            )?;
            let mut aggregate: AudioObjectID = UNKNOWN_OBJECT;
            // SAFETY: valid dictionary and out pointer.
            check(
                unsafe {
                    AudioHardwareCreateAggregateDevice(&props, NonNull::from(&mut aggregate))
                },
                "creating the aggregate device",
            )?;
            Ok(aggregate)
        })();
        let aggregate = match built {
            Ok(a) => a,
            Err(e) => {
                destroy_tap();
                return Err(e);
            }
        };
        let destroy_all = |aggregate: AudioObjectID| {
            // SAFETY: both objects were created above.
            unsafe {
                let _ = AudioHardwareDestroyAggregateDevice(aggregate);
                let _ = AudioHardwareDestroyProcessTap(tap);
            }
        };

        // 4. IOProc feeding the ring.
        let context = Box::into_raw(Box::new(CallbackContext {
            writer,
            channels,
            interleaved,
            scratch: vec![0.0; SCRATCH_FRAMES * channels],
        }));
        let mut proc_id: AudioDeviceIOProcID = None;
        // SAFETY: the callback matches the required signature and the context outlives it.
        let status = unsafe {
            AudioDeviceCreateIOProcID(
                aggregate,
                Some(io_proc),
                context.cast::<c_void>(),
                NonNull::from(&mut proc_id),
            )
        };
        if status != 0 {
            // SAFETY: the IOProc was not created, so nothing else references the context.
            drop(unsafe { Box::from_raw(context) });
            destroy_all(aggregate);
            return Err(backend_error("creating the IOProc", status));
        }
        // SAFETY: `proc_id` was just created for `aggregate`.
        let status = unsafe { AudioDeviceStart(aggregate, proc_id) };
        if status != 0 {
            // SAFETY: destroy the IOProc first, then nothing references the context.
            unsafe {
                let _ = AudioDeviceDestroyIOProcID(aggregate, proc_id);
                drop(Box::from_raw(context));
            }
            destroy_all(aggregate);
            return Err(backend_error("starting the aggregate device", status));
        }

        Ok(MacTap {
            aggregate,
            tap,
            io_proc: proc_id,
            context,
            info: StreamInfo {
                device_name: SYSTEM_AUDIO_NAME.to_string(),
                device_id: SYSTEM_AUDIO_ID.to_string(),
                sample_rate,
                channels: channels as u16,
                buffer_frames: None,
            },
        })
    }
}

impl Drop for MacTap {
    fn drop(&mut self) {
        // SAFETY: stop and destroy the IOProc before freeing what it points to, then remove the
        // aggregate device and the tap. Failures are ignored: there is nothing left to do in Drop.
        unsafe {
            let _ = AudioDeviceStop(self.aggregate, self.io_proc);
            let _ = AudioDeviceDestroyIOProcID(self.aggregate, self.io_proc);
            drop(Box::from_raw(self.context));
            let _ = AudioHardwareDestroyAggregateDevice(self.aggregate);
            let _ = AudioHardwareDestroyProcessTap(self.tap);
        }
    }
}
