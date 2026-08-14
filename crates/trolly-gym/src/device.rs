//! Training-device selection for the `tch` / libtorch stack.
//!
//! ROCm/HIP libtorch builds expose AMD GPUs through the same `Device::Cuda`
//! API as NVIDIA CUDA. Prefer [`resolve_training_device`] from env
//! (`TROLLY_TRAIN_DEVICE`) rather than hard-coding `Device::Cpu`.

use tch::Device;

use crate::orchestrator::DeviceSpec;

/// Resolve `TROLLY_TRAIN_DEVICE` to a libtorch device.
///
/// `auto` uses CUDA/ROCm when `tch::Cuda::is_available()`, else CPU.
/// An explicit `cuda` / `cuda:N` request fails if no GPU is visible.
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

pub fn gpu_available() -> bool {
    tch::Cuda::is_available()
}

pub fn describe_device(device: Device) -> String {
    match device {
        Device::Cpu => "cpu".to_string(),
        Device::Cuda(index) => format!("cuda:{index} (libtorch CUDA API; ROCm/HIP on AMD)"),
        other => format!("{other:?}"),
    }
}
