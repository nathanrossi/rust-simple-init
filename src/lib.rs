pub mod services;
pub mod procfs;
pub mod sysfs;
pub mod kmod;
pub mod configfs;
pub mod uevent;
pub mod runtime;
pub mod service;
pub mod logging;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
