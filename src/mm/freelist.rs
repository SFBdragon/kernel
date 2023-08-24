use alloc::collections::linked_list::LinkedList;
use core::alloc::AllocError;
use core::cmp::Ordering;

use align_address::Align;

#[derive(Debug)]
pub struct FreeListEntry {
	pub start: usize,
	pub end: usize,
}

impl FreeListEntry {
	pub const fn new(start: usize, end: usize) -> Self {
		FreeListEntry { start, end }
	}
}

#[derive(Debug)]
pub struct FreeList {
	pub list: LinkedList<FreeListEntry>,
}

trait LinkedListExt<T> {
	fn insert(&mut self, index: usize, element: T);
	fn get(&self, index: usize) -> Option<&T>;
	fn get_mut(&mut self, index: usize) -> Option<&mut T>;
}

impl<T> LinkedListExt<T> for LinkedList<T> {
	fn insert(&mut self, index: usize, element: T) {
		let mut split = self.split_off(index);
		self.push_back(element);
		self.append(&mut split);
	}

	fn get(&self, index: usize) -> Option<&T> {
		self.iter()
			.enumerate()
			.find(|(i, _)| *i == index)
			.map(|(_, element)| element)
	}

	fn get_mut(&mut self, index: usize) -> Option<&mut T> {
		self.iter_mut()
			.enumerate()
			.find(|(i, _)| *i == index)
			.map(|(_, element)| element)
	}
}

impl FreeList {
	pub const fn new() -> Self {
		Self {
			list: LinkedList::new(),
		}
	}

	pub fn allocate(&mut self, size: usize, alignment: Option<usize>) -> Result<usize, AllocError> {
		trace!(
			"Allocating {} bytes from Free List {:#X}",
			size,
			self as *const Self as usize
		);

		let new_size = if let Some(align) = alignment {
			size + align
		} else {
			size
		};

		// Find a region in the Free List that has at least the requested size.
		for (i, node) in self.list.iter_mut().enumerate() {
			let (region_start, region_size) = (node.start, node.end - node.start);

			match region_size.cmp(&new_size) {
				Ordering::Greater => {
					// We have found a region that is larger than the requested size.
					// Return the address to the beginning of that region and shrink the region by that size.
					if let Some(align) = alignment {
						let new_addr = region_start.align_up(align);
						node.start += size + (new_addr - region_start);
						if new_addr != region_start {
							let new_entry = FreeListEntry::new(region_start, new_addr);
							self.list.insert(i, new_entry);
						}
						return Ok(new_addr);
					} else {
						node.start += size;
						return Ok(region_start);
					}
				}
				Ordering::Equal => {
					// We have found a region that has exactly the requested size.
					// Return the address to the beginning of that region and move the node into the pool for deletion or reuse.
					if let Some(align) = alignment {
						let new_addr = region_start.align_up(align);
						if new_addr != region_start {
							node.end = new_addr;
						}
						return Ok(new_addr);
					} else {
						self.list.remove(i);
						return Ok(region_start);
					}
				}
				Ordering::Less => {}
			}
		}

		Err(AllocError)
	}

	#[cfg(all(target_arch = "x86_64", not(feature = "pci")))]
	pub fn reserve(&mut self, address: usize, size: usize) -> Result<(), AllocError> {
		trace!(
			"Try to reserve {} bytes at {:#X} from Free List {:#X}",
			size,
			address,
			self as *const Self as usize
		);

		// Find a region in the Free List that has at least the requested size.
		for (i, node) in self.list.iter_mut().enumerate() {
			let (region_start, region_size) = (node.start, node.end - node.start);

			if address > region_start && address + size < region_start + region_size {
				node.start = address + size;
				let new_entry = FreeListEntry::new(region_start, address);
				self.list.insert(i, new_entry);
				return Ok(());
			} else if address > region_start && address + size == region_start + region_size {
				node.start = address + size;
				return Ok(());
			} else if address == region_start && address + size < region_start + region_size {
				node.start = region_start + size;
				return Ok(());
			}
		}

		Err(AllocError)
	}

	pub fn deallocate(&mut self, address: usize, size: usize) {
		trace!(
			"Deallocating {} bytes at {:#X} from Free List {:#X}",
			size,
			address,
			self as *const Self as usize
		);

		let end = address + size;

		for (i, node) in self.list.iter_mut().enumerate() {
			let (region_start, region_end) = (node.start, node.end);

			if region_start == end {
				// The deallocated memory extends this free memory region to the left.
				node.start = address;

				// Check if it can even reunite with the previous region.
				if i > 0 {
					if let Some(prev_node) = self.list.get_mut(i - 1) {
						let prev_region_end = prev_node.end;

						if prev_region_end == address {
							// It can reunite, so let the current region span over the reunited region and move the duplicate node
							// into the pool for deletion or reuse.
							prev_node.end = region_end;
							self.list.remove(i);
						}
					}
				}

				return;
			} else if region_end == address {
				node.end = end;

				// Check if it can even reunite with the next region.
				if let Some(next_node) = self.list.get_mut(i + 1) {
					let next_region_start = next_node.start;

					if next_region_start == end {
						// It can reunite, so let the current region span over the reunited region and move the duplicate node
						// into the pool for deletion or reuse.
						next_node.start = region_start;
						self.list.remove(i);
					}
				}

				return;
			} else if end < region_start {
				// The deallocated memory does not extend any memory region and needs an own entry in the Free List.
				// Get that entry from the node pool.
				// We search the list from low to high addresses and insert us before the first entry that has a
				// higher address than us.
				let new_entry = FreeListEntry::new(address, end);
				self.list.insert(i, new_entry);
				return;
			}
		}

		// We could not find an entry with a higher address than us.
		// So we become the new last entry in the list. Get that entry from the node pool.
		let new_element = FreeListEntry::new(address, end);
		self.list.push_back(new_element);
	}

	pub fn print_information(&self, header: &str) {
		infoheader!(header);

		for node in self.list.iter() {
			info!("{:#016X} - {:#016X}", node.start, node.end);
		}

		infofooter!();
	}
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
	use super::*;

	#[test]
	fn add_element() {
		let mut freelist = FreeList::new();
		let entry = FreeListEntry::new(0x10000, 0x100000);

		freelist.list.push_back(entry);

		let mut cursor = freelist.list.cursor_front_mut();

		while let Some(node) = cursor.peek_next() {
			assert!(node.start != 0x1000);
			assert!(node.end != 0x10000);

			cursor.move_next();
		}
	}

	#[test]
	fn allocate() {
		let mut freelist = FreeList::new();
		let entry = FreeListEntry::new(0x10000, 0x100000);

		freelist.list.push_back(entry);
		let addr = freelist.allocate(0x1000, None);

		assert_eq!(addr.unwrap(), 0x10000);

		let mut cursor = freelist.list.cursor_front_mut();
		while let Some(node) = cursor.current() {
			assert_eq!(node.start, 0x11000);
			assert_eq!(node.end, 0x100000);

			cursor.move_next();
		}

		let addr = freelist.allocate(0x1000, Some(0x2000));
		let mut cursor = freelist.list.cursor_front_mut();
		assert!(cursor.current().is_some());
		if let Some(node) = cursor.current() {
			assert_eq!(node.start, 0x11000);
		}

		cursor.move_next();
		assert!(cursor.current().is_some());
		if let Some(node) = cursor.current() {
			assert_eq!(node.start, 0x13000);
		}
	}

	#[test]
	fn deallocate() {
		let mut freelist = FreeList::new();
		let entry = FreeListEntry::new(0x10000, 0x100000);

		freelist.list.push_back(entry);
		let addr = freelist.allocate(0x1000, None);
		freelist.deallocate(addr.unwrap(), 0x1000);

		let mut cursor = freelist.list.cursor_front_mut();
		while let Some(node) = cursor.current() {
			assert_eq!(node.start, 0x10000);
			assert_eq!(node.end, 0x100000);

			cursor.move_next();
		}
	}
}
