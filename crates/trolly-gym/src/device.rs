//! Training-device selection for the `tch` / libtorch stack.
//!
//! ROCm/HIP libtorch builds expose AMD GPUs through the same `Device::Cuda`
//! API as NVIDIA CUDA. The GPU orchestrator refuses CPU and the iGPU.
//! Prefer [`require_rx_training_device`].

use std::ffi::CStr;
use std::os::raw::c_char;
use tch::Device;

use crate::orchestrator::DeviceSpec;

/// First field of `cudaDeviceProp` / `hipDeviceProp_t` as used by ATen.
#[repr(C)]
struct AtCudaDeviceProp {
    name: [u8; 256],
}

#[derive(Clone, Debug)]
struct HipGpu {
    index: usize,
    name: String,
    arch: String,
}

impl HipGpu {
    fn label(&self) -> String {
        if self.arch.is_empty() {
            format!("cuda:{}={}", self.index, self.name)
        } else {
            format!("cuda:{}={} ({})", self.index, self.name, self.arch)
        }
    }
}

// HIP-only export. GNU ld --as-needed drops libtorch_hip unless a symbol from
// it is referenced, after which tch::Cuda::is_available reports false on ROCm.
#[link(name = "torch_hip")]
extern "C" {
    fn aoti_torch_create_cuda_guard();
    /// `at::cuda::getDeviceProperties(c10::DeviceIndex)` — DeviceIndex is int8.
    #[link_name = "_ZN2at4cuda19getDevicePropertiesEa"]
    fn at_cuda_get_device_properties(device: i8) -> *const AtCudaDeviceProp;
}

/// Substring matched against HIP marketing name and `gcnArchName`.
/// Override with `TROLLY_TRAIN_GPU_MATCH`. Default `RX` means a discrete
/// Radeon RX card (marketing name contains `RX`, or ISA `gfx1102` for the
/// RX 7600). The iGPU is `gfx1036` / "AMD Radeon Graphics".
pub fn required_gpu_name_match() -> String {
    std::env::var("TROLLY_TRAIN_GPU_MATCH")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "RX".into())
}

/// True when `text` contains the required match as a case-insensitive substring.
pub fn name_matches_required_gpu(text: &str, match_substr: &str) -> bool {
    if match_substr.is_empty() {
        return false;
    }
    text.to_ascii_uppercase()
        .contains(&match_substr.to_ascii_uppercase())
}

/// Default `RX` match: marketing name contains RX, or Navi 33 ISA (RX 7600).
pub fn device_is_rx_card(name: &str, arch: &str, match_substr: &str) -> bool {
    if name_matches_required_gpu(name, match_substr)
        || name_matches_required_gpu(arch, match_substr)
    {
        return true;
    }
    match_substr.eq_ignore_ascii_case("RX")
        && arch.to_ascii_lowercase().starts_with("gfx1102")
}

/// Resolve `TROLLY_TRAIN_DEVICE` to a libtorch device.
///
/// `auto` uses CUDA/ROCm when `tch::Cuda::is_available()`, else CPU.
/// An explicit `cuda` / `cuda:N` request fails if no GPU is visible.
/// The weekday trainer must use [`require_rx_training_device`] instead.
pub fn resolve_training_device() -> Result<Device, String> {
    resolve_device(DeviceSpec::from_env())
}

pub fn resolve_device(spec: DeviceSpec) -> Result<Device, String> {
    match spec {
        DeviceSpec::Cpu => Ok(Device::Cpu),
        DeviceSpec::Auto => Ok(if gpu_available() {
            Device::Cuda(0)
        } else {
            Device::Cpu
        }),
        DeviceSpec::Cuda(index) => {
            if !gpu_available() {
                return Err(format!(
                    "TROLLY_TRAIN_DEVICE={spec} but libtorch reports no CUDA/ROCm GPU \
                     (install ROCm userspace + a ROCm libtorch/PyTorch build, and ensure \
                     the user is in the render/video groups)"
                ));
            }
            let count = tch::Cuda::device_count() as usize;
            if index >= count {
                return Err(format!(
                    "TROLLY_TRAIN_DEVICE={spec} but only {count} GPU(s) are visible"
                ));
            }
            Ok(Device::Cuda(index))
        }
    }
}

/// Resolve a training device and bail unless it is the discrete RX card.
///
/// CPU, missing HIP, and the iGPU (`gfx1036`) are errors. `auto` searches
/// visible HIP devices; `cuda:N` must itself be the RX card.
pub fn require_rx_training_device(spec: DeviceSpec) -> Result<Device, String> {
    std::hint::black_box(aoti_torch_create_cuda_guard as *const ());
    if matches!(spec, DeviceSpec::Cpu) {
        return Err(
            "training refuses CPU (TROLLY_TRAIN_DEVICE=cpu); the discrete RX GPU is required"
                .into(),
        );
    }
    if !gpu_available() {
        return Err(
            "training bails: libtorch reports no CUDA/ROCm GPU (need render/video groups \
             and a gfx1102 ROCm wheel on the RX card)"
                .into(),
        );
    }
    let match_substr = required_gpu_name_match();
    let named = list_hip_devices()?;
    if named.is_empty() {
        return Err("training bails: HIP device count is 0".into());
    }
    let inventory = named
        .iter()
        .map(HipGpu::label)
        .collect::<Vec<_>>()
        .join(", ");

    let candidates: Vec<HipGpu> = match spec {
        DeviceSpec::Cuda(index) => named.into_iter().filter(|g| g.index == index).collect(),
        DeviceSpec::Auto | DeviceSpec::Cpu => named,
    };

    if let DeviceSpec::Cuda(_) = spec {
        if candidates.is_empty() {
            return Err(format!(
                "training bails: TROLLY_TRAIN_DEVICE={spec} but HIP has no such device ({inventory})"
            ));
        }
    }

    for gpu in &candidates {
        if device_is_rx_card(&gpu.name, &gpu.arch, &match_substr) {
            return Ok(Device::Cuda(gpu.index));
        }
    }

    Err(format!(
        "training bails: no visible device is the RX card (match `{match_substr}` / gfx1102; \
         refuses CPU and iGPU). Visible: {inventory}"
    ))
}

pub fn gpu_available() -> bool {
    std::hint::black_box(aoti_torch_create_cuda_guard as *const ());
    tch::Cuda::is_available()
}

pub fn describe_device(device: Device) -> String {
    match device {
        Device::Cpu => "cpu".to_string(),
        Device::Cuda(index) => match hip_gpu(index) {
            Ok(gpu) => format!(
                "cuda:{index} {} {} (libtorch CUDA API; ROCm/HIP on AMD)",
                gpu.name, gpu.arch
            ),
            Err(_) => {
                format!("cuda:{index} (libtorch CUDA API; ROCm/HIP on AMD)")
            }
        },
        other => format!("{other:?}"),
    }
}

fn list_hip_devices() -> Result<Vec<HipGpu>, String> {
    let count = tch::Cuda::device_count();
    if count <= 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        out.push(hip_gpu(i as usize)?);
    }
    Ok(out)
}

fn hip_gpu(index: usize) -> Result<HipGpu, String> {
    std::hint::black_box(aoti_torch_create_cuda_guard as *const ());
    if index > i8::MAX as usize {
        return Err(format!("CUDA device index {index} is out of range"));
    }
    let ptr = unsafe { at_cuda_get_device_properties(index as i8) };
    if ptr.is_null() {
        return Err(format!(
            "at::cuda::getDeviceProperties({index}) returned null"
        ));
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, 4096) };
    let name = unsafe { CStr::from_ptr((*ptr).name.as_ptr() as *const c_char) }
        .to_string_lossy()
        .into_owned();
    if name.is_empty() {
        return Err(format!("device {index} has an empty HIP name"));
    }
    Ok(HipGpu {
        index,
        name,
        arch: gcn_arch_from_prop(bytes),
    })
}

fn gcn_arch_from_prop(bytes: &[u8]) -> String {
    let Some(pos) = bytes.windows(3).position(|w| w == b"gfx") else {
        return String::new();
    };
    let slice = &bytes[pos..];
    let end = slice
        .iter()
        .position(|&b| b == 0 || !(b.is_ascii_alphanumeric()))
        .unwrap_or(slice.len().min(16));
    String::from_utf8_lossy(&slice[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{device_is_rx_card, name_matches_required_gpu};

    #[test]
    fn rx_7600_matches_rx() {
        assert!(name_matches_required_gpu("AMD Radeon RX 7600", "RX"));
        assert!(device_is_rx_card("AMD Radeon RX 7600", "gfx1102", "RX"));
    }

    #[test]
    fn graphics_plus_gfx1102_is_rx_7600() {
        // This HIP stack names both GPUs "AMD Radeon Graphics"; ISA distinguishes them.
        assert!(device_is_rx_card("AMD Radeon Graphics", "gfx1102", "RX"));
    }

    #[test]
    fn igpu_gfx1036_is_not_rx() {
        assert!(!device_is_rx_card("AMD Radeon Graphics", "gfx1036", "RX"));
        assert!(!name_matches_required_gpu("AMD Radeon Graphics", "RX"));
    }

    #[test]
    fn empty_match_never_passes() {
        assert!(!device_is_rx_card("AMD Radeon RX 7600", "gfx1102", ""));
    }
}
