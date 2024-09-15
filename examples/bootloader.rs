use std::io;

use rust_simple_init::runtime::Runtime;
use rust_simple_init::service::ServiceManager;
use rust_simple_init::services::mount;
use rust_simple_init::services::console::ConsoleService;
use rust_simple_init::services::dev::DeviceManagerService;
use rust_simple_init::services::bootloader::Bootloader;
use rust_simple_init::logging::Logger;

pub fn main() -> std::result::Result<(), Box<dyn std::error::Error>>
{
	let mut logger = Logger::new();
	logger.add(io::stdout());
	logger.service_log("init", "started");

	logger.service_log("init", "setting hostname");
	if let Err(_) = nix::unistd::sethostname("rpi") {
		logger.service_log("init", "failed to set hostname");
	}

	let mut manager = ServiceManager::new();
	let mut rt = Runtime::new(logger).unwrap();

	{
		let mut service = mount::MountSetup::new();
		// procfs is needed first in order to check mounts
		service.add("proc", Some("proc"), "/proc", None);
		service.add("sysfs", Some("sysfs"), "/sys", None);
		// device nodes
		service.add("devtmpfs", None, "/dev", Some("mode=0755"));
		// /dev/pts and /dev/ptmx
		service.add("devpts", Some("devpts"), "/dev/pts", Some("mode=0620,ptmxmode=0666,gid=5"));
		// setup later mounts
		service.add("tmpfs", Some("tmpfs"), "/run", Some("mode=0755,nodev,nosuid,strictatime"));
		service.add("tmpfs", Some("tmpfs"), "/var/volatile", None);
		// kernel debug
		service.add("debugfs", None, "/sys/kernel/debug", None);

		let instance = manager.add_service(&mut rt, service, true);
		rt.poll_service_ready(&mut manager, &instance)?; // wait for service to complete
	}

	rt.logger.service_log("init", "initial mounts complete");

	// requires the /var/volatile mount
	rt.logger.add_file("/var/volatile/log/messages")?;
	rt.logger.service_log("init", "created log file for messages");

	// start device manager
	manager.add_service(&mut rt, DeviceManagerService::new(), true);

	// add serial consoles
	// manager.add_service(&mut rt, ConsoleService::new("ttyACM0", 115200, true), true);
	// manager.add_service(&mut rt, ConsoleService::new("ttyAMA0", 115200, true), true);
	// manager.add_service(&mut rt, ConsoleService::new("ttyUSB0", 115200, true), true);
	manager.add_service(&mut rt, ConsoleService::new("ttyS0", 115200, true), true);

	// Start the boot loading service that discovers boot sources
	manager.add_service(&mut rt, Bootloader::new(), true);

	return rt.poll(&mut manager, false);
}

