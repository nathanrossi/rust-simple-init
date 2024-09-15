use std::path::Path;
use std::path::PathBuf;
use std::os::linux::fs::MetadataExt;
use std::process::Command;
use super::super::*;
use service::{Service, ServiceEvent, ServiceState};
use runtime::Runtime;
use procfs;
use crate::Result;

#[derive(Debug, Clone)]
struct BootEntry
{
	kernel : PathBuf,
	initramfs : Option<PathBuf>,
	append : Option<String>,
}

enum BlockState
{
	Unchecked,
	Mounting(std::process::Child),
	Scanning,
	Complete,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockDeviceType
{
	Other,
	Internal,
	USB,
	Network,
}

struct DeviceProbe
{
	name : String,
	device : PathBuf,
	devicetype : BlockDeviceType,
	point : PathBuf,
	state : BlockState,
	entries : Vec<BootEntry>,
}

pub fn file_is_chardev<P: AsRef<std::path::Path>>(path : P, major : u64, minor : u64) -> bool {
	if let Ok(meta) = path.as_ref().metadata() {
		let mode = meta.st_mode();
		if !((mode & libc::S_IFBLK != 0) || (mode & libc::S_IFCHR != 0)) {
			return false;
		}

		let dev = meta.st_rdev();
		if nix::sys::stat::major(dev) != major {
			return false;
		}
		if nix::sys::stat::minor(dev) != minor {
			return false;
		}
		return true;
	}
	return false;
}

fn block_device_is_storage(name : &str) -> bool {
	// Filter out any block devices that are not typically providing storage (e.g. loop/ram)
	let mut sysfs = PathBuf::from("/sys/block");
	sysfs.push(name);
	sysfs.push("device");

	if !sysfs.exists() {
		return false;
	}
	return true;
}

pub fn block_device_has_partitions(name : &str) -> bool {
	let sysfs = PathBuf::from("/sys/class/block");
	if let Ok(entries) = sysfs.join(name).read_dir() {
		for i in entries {
			if let Ok(entry) = i {
				let partname = entry.file_name().into_string().unwrap();
				if !partname.starts_with(&name) {
					continue;
				}

				return true;
			}
		}
	}
	return false;
}

pub fn block_device_path(name : &str) -> Option<PathBuf> {
	let sysfs = PathBuf::from("/sys/class/block");
	if let Ok(block) = sysfs.join(name).canonicalize() {
		// The block device is the parent block device
		let path = block.join("device");
		if path.exists() {
			return Some(path);
		}

		// Check if this is a partition block device, and has a parent
		let path = block.join("../device");
		if path.exists() {
			return Some(path);
		}
		return None;
	}
	return None;
}

pub fn block_device_type(name : &str) -> BlockDeviceType {
	if let Some(devicepath) = block_device_path(name) {
		// Read the subsystem
		if let Some(subsystem) = sysfs::read_link_file_name(devicepath.join("subsystem")) {
			// println!("subsystem = {:?}", subsystem);
			if subsystem == "virtio" {
				return BlockDeviceType::Internal;
			} else if subsystem == "scsi" {
				if let Some(host) = sysfs::walk_path_has_subdir(devicepath, "scsi_host") {
					if let Some(subsystem) = sysfs::read_link_file_name(host.join("../subsystem")) {
						if subsystem == "usb" {
							return BlockDeviceType::USB;
						}
					}
				}
				return BlockDeviceType::Internal;
			}
		}
		return BlockDeviceType::Other;
	}
	// No backing device
	return BlockDeviceType::Other;
}

pub fn block_device_node(name : &str) -> Result<PathBuf> {
	// Block devices under /sys/block do not contain partition devices
	let devinfo =  Path::new("/sys/class/block").join(name).join("dev");
	let chardev = crate::sysfs::read_line_file(devinfo);
	if let Some(chardev) = chardev {
		if let Some((major, minor)) = chardev.split_once(":") {
			let major = u64::from_str_radix(&major, 10)?;
			let minor = u64::from_str_radix(&minor, 10)?;

			// Quickly check if the "/dev/<name>" node matches the chardev
			let default = Path::new("/dev").join(name);
			if file_is_chardev(&default, major, minor) {
				return Ok(default);
			}

			// Search all files in /dev/
			if let Ok(entries) = std::fs::read_dir("/dev") {
				for i in entries {
					if let Ok(entry) = i {
						if entry.path().is_dir() {
							continue;
						}

						if file_is_chardev(&entry.path(), major, minor) {
							return Ok(entry.path());
						}
					}
				}
			}
			return Err("Unable to find associated character device in filesystem".into());
		}
		return Err("Unable to parse dev node information".into());
	}
	return Err("Missing dev node information".into());
}

impl DeviceProbe
{
	fn new(name : &str) -> Result<Self> {
		let chardev = block_device_node(&name)?;
		let point = PathBuf::from("/var/run/bootloader/mounts").join(&name);
		let devicetype = block_device_type(name);
		return Ok(Self {
			name : name.to_owned(),
			device : chardev,
			devicetype : devicetype,
			point : point,
			state : BlockState::Unchecked,
			entries : Vec::new(),
		});
	}

	fn mount(&mut self, runtime : &mut Runtime) -> crate::Result<()> {
		// Check if already mounted
		if procfs::device_mounted(&self.device) {
			return Err("Device already mounted".into());
		}

		// Check/make the mount point
		std::fs::create_dir_all(&self.point)?;

		let mut command = Command::new("mount");
		command
			.arg(&self.device)
			.arg(&self.point)
			.arg("-o").arg("ro");

		if let Ok(child) = command.spawn() {
			runtime.logger.service_log("bootloader", &format!("mounting {:?} at {:?}", self.device, self.point));
			self.state = BlockState::Mounting(child);
			return Ok(());
		} else {
			runtime.logger.service_log("bootloader", &format!("mounting failed for {:?}", self.device));
			self.state = BlockState::Complete;
			return Ok(());
		}
	}

	fn scan(&mut self, runtime : &mut Runtime) {
		// search for the EFI boot files
		let default = "bootx64.efi".to_lowercase();
		let subdir = PathBuf::from("EFI");

		if let Ok(entries) = self.point.join(&subdir).read_dir() {
			for i in entries {
				if let Ok(entry) = i {
					if entry.path().is_dir() {
						continue;
					}
					let filename = entry.file_name();

					if let Some(filename) = filename.to_str() {
						if filename.to_lowercase() == default {
							let subpath = subdir.join(filename);
							self.state = BlockState::Complete;
							runtime.logger.service_log("bootloader", &format!("Scan of {} found bootable at {:?}", self.name, entry.path()));
							self.entries.push(BootEntry {
								kernel : subpath,
								initramfs : None,
								append : None,
							});
							return;
						}
					}
				}
			}
		}

		runtime.logger.service_log("bootloader", &format!("Scan complete of {}, nothing found to boot", self.name));
		self.state = BlockState::Complete;
	}

	fn event(&mut self, runtime : &mut Runtime, event : &ServiceEvent) -> bool {
		match event {
			ServiceEvent::ProcessExited(pid, status) => {
				match &self.state {
					BlockState::Mounting(child) => {
						if child.id() != *pid {
							return false;
						}

						// Mount process has finished
						if !status.success() {
							runtime.logger.service_log("bootloader", &format!("Mount failed for {}, return code = {}", self.name, status));
							self.state = BlockState::Complete;
						} else {
							runtime.logger.service_log("bootloader", &format!("Mount completed for {}", self.name));
							self.state = BlockState::Scanning;
							self.scan(runtime);
						}
						return true;
					},
					_ => {},
				}
			},
			ServiceEvent::Device(_) => {
				// runtime.logger.service_log("bootloader", &format!("got dev event {:?}", &dev));
				return false;
			},
			_ => {},
		}
		return false;
	}
}

pub struct Bootloader
{
	checked : Vec<DeviceProbe>,
}

impl Bootloader
{
	pub fn new() -> Self {
		return Self {
			checked : Vec::new(),
			};
	}

	fn probe_partition(&mut self, runtime : &mut Runtime, name : String) -> bool {
		for i in &self.checked {
			// already exists
			if i.name == name {
				return false;
			}
		}

		match DeviceProbe::new(&name) {
			Ok(mut block) => {
				runtime.logger.service_log("bootloader", &format!("found existing block device {} to probe", &block.name));
				match block.mount(runtime) {
					Ok(_) => {
						self.checked.push(block);
					},
					Err(e) => {
						runtime.logger.service_log("bootloader", &format!("Error mounting block device {}, {:?}", &name, e));
					},
				}
				return true;
			},
			Err(e) => {
				runtime.logger.service_log("bootloader", &format!("Error probing block device {}, {:?}", &name, e));
				return false;
			},
		}
	}

	fn select_boot_entry(&self, order : &[BlockDeviceType]) -> Option<BootEntry> {
		for t in order {
			for i in &self.checked {
				// already exists
				match i.state {
					BlockState::Complete => {
						if i.devicetype == *t && i.entries.len() != 0{
							return Some(i.entries[0].clone());
						}
					},
					_ => {},
				}
			}
		}
		return None;
	}
}

impl Service for Bootloader
{
	fn setup(&mut self, _runtime : &mut Runtime) {}

	fn state(&self) -> ServiceState {
		if self.checked.len() == 0 {
			return ServiceState::Inactive;
		}
		return ServiceState::Running;
	}

	fn start(&mut self, runtime : &mut Runtime) {
		runtime.logger.service_log("bootloader", "starting search of block devices");

		if let Ok(entries) = std::fs::read_dir("/sys/block") {
			for i in entries {
				if let Ok(entry) = i {
					let name = entry.file_name().into_string().unwrap();

					if !block_device_is_storage(&name) {
						continue;
					}

					// partitions
					let mut partitions = false;
					if let Ok(entries) = entry.path().read_dir() {
						for i in entries {
							if let Ok(entry) = i {
								let partname = entry.file_name().into_string().unwrap();
								if !partname.starts_with(&name) {
									continue;
								}

								// found partition
								partitions |= self.probe_partition(runtime, partname);
							}
						}
					}

					if !partitions {
						// handle stand alone block devices without partitions (e.g. cd/iso)
					}
				}
			}
		}
	}

	fn stop(&mut self, _runtime : &mut Runtime) {
		// TODO
	}

	fn event(&mut self, runtime : &mut Runtime, event : ServiceEvent) -> bool {
		let mut handled = false;
		for i in &mut self.checked {
			handled |= i.event(runtime, &event);
		}

		if handled {
			return true;
		}

		// Monitor for new block devices
		match event {
			ServiceEvent::Device(dev) => {
				if let Some(subsystem) = dev.get("SUBSYSTEM") {
					if subsystem == "block" {
						if dev.action() == crate::uevent::EventAction::Add {
							if let Some(devname) = dev.get("DEVNAME") {
								if !block_device_has_partitions(devname) {
									runtime.logger.service_log("bootloader", &format!("new block device {}", devname));
									self.probe_partition(runtime, devname.to_owned());
								}
							}
						}
					}
				}
				return false;
			},
			_ => {},
		}
		return false;
	}
}
