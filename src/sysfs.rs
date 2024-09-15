use std::io;
use std::io::{BufRead, Read, BufReader};
use std::path::Path;
use std::path::PathBuf;

pub trait SysfsEntryParsable<T>
{
	fn parse(line : &str) -> Option<T>;
}

pub struct SysfsEntryIter<R, T>
{
	file : std::io::BufReader<R>,
	evaluator : std::marker::PhantomData<T>,
}

impl<R: Read, T: SysfsEntryParsable<T>> Iterator for SysfsEntryIter<R, T>
{
	type Item = T;

	fn next(&mut self) -> Option<T>
	{
		let mut buffer : String = String::with_capacity(512);
		loop {
			buffer.clear();
			if let Ok(count) = self.file.read_line(&mut buffer) {
				if count == 0 {
					return None; // EOF
				}

				if buffer.len() == 0 {
					continue;
				}

				if let Some(entry) = T::parse(&buffer.trim()) {
					return Some(entry);
				}
			} else {
				return None;
			}
		}
	}
}

impl<T: SysfsEntryParsable<T>> SysfsEntryIter<std::fs::File, T>
{
	pub fn from_file(path : &str) -> io::Result<SysfsEntryIter<std::fs::File, T>>
	{
		return Ok(SysfsEntryIter {
				file : BufReader::new(std::fs::File::open(path)?),
				evaluator : std::marker::PhantomData,
			});
	}
}

impl<T: SysfsEntryParsable<T>> SysfsEntryIter<&[u8], T>
{
	pub fn from_string(s : &str) -> SysfsEntryIter<&[u8], T>
	{
		return SysfsEntryIter {
				file : BufReader::new(s.as_bytes()),
				evaluator : std::marker::PhantomData,
			};
	}
}

pub fn read_file<P: AsRef<Path>>(path : P) -> Option<String>
{
	if let Ok(content) = std::fs::read_to_string(path) {
		return Some(content);
	}
	return None;
}

pub fn read_line_file<P: AsRef<Path>>(path : P) -> Option<String>
{
	if let Ok(content) = std::fs::read_to_string(path) {
		if let Some(line) = content.lines().next() {
			return Some(line.to_string());
		}
	}
	return None;
}

pub fn read_link<P: AsRef<Path>>(path : P) -> Option<PathBuf> {
	if let Ok(path) = std::fs::read_link(path) {
		return Some(PathBuf::from(path));
	}
	return None;
}

pub fn read_link_file_name<P: AsRef<Path>>(path : P) -> Option<String> {
	if let Ok(path) = std::fs::read_link(path) {
		if let Some(filename) = PathBuf::from(path).file_name() {
			if let Some(filename) = filename.to_str() {
				return Some(filename.to_owned());
			}
		}
	}
	return None;
}

fn path_has_subdir<P: AsRef<Path>>(path : P, subdir : &str) -> bool {
	if let Ok(entries) = path.as_ref().read_dir() {
		for i in entries {
			if let Ok(entry) = i {
				if entry.file_name() == subdir {
					return true;
				}
			}
		}
	}
	return false;
}

pub fn walk_path_has_subdir<P: AsRef<Path>>(path : P, subdir : &str) -> Option<PathBuf> {
	let mut base = path.as_ref().to_path_buf();
	loop {
		if path_has_subdir(&base, subdir) {
			return Some(base);
		}

		if base.pop() {
			// check the next parent
			continue;
		}
		return None;
	}
}
