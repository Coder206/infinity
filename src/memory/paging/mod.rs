//! # Paging
//! Some code was borrowed from [Phil Opp's Blog](http://os.phil-opp.com/modifying-page-tables.html)

pub use self::entry::*;
use memory::{PAGE_SIZE, Frame, FrameAllocator};
use self::temporary_page::TemporaryPage;
pub use self::mapper::Mapper;
use core::ops::{Deref, DerefMut};
use multiboot2::BootInformation;

mod entry;
mod mapper;
mod table;
mod temporary_page;

const ENTRY_COUNT: usize = 512;

pub type PhysicalAddress = usize;
pub type VirtualAddress = usize;

#[derive(Debug, Clone, Copy)]
pub struct Page {
    number: usize,
}

impl Page {
    pub fn containing_address(address: VirtualAddress) -> Page {
        assert!(address < 0x0000_8000_0000_0000 || address >= 0xffff_8000_0000_0000,
                "invalid address: 0x{:x}",
                address);
        Page { number: address / PAGE_SIZE }
    }

    fn start_address(&self) -> usize {
        self.number * PAGE_SIZE
    }

    fn p4_index(&self) -> usize {
        (self.number >> 27) & 0o777
    }
    fn p3_index(&self) -> usize {
        (self.number >> 18) & 0o777
    }
    fn p2_index(&self) -> usize {
        (self.number >> 9) & 0o777
    }
    fn p1_index(&self) -> usize {
        (self.number >> 0) & 0o777
    }
}

pub struct ActivePageTable {
    mapper: Mapper,
}

impl Deref for ActivePageTable {
    type Target = Mapper;

    fn deref(&self) -> &Mapper {
        &self.mapper
    }
}

impl DerefMut for ActivePageTable {
    fn deref_mut(&mut self) -> &mut Mapper {
        &mut self.mapper
    }
}

impl ActivePageTable {
    unsafe fn new() -> ActivePageTable {
        ActivePageTable { mapper: Mapper::new() }
    }

    pub fn with<F>(&mut self,
                   table: &mut InactivePageTable,
                   temporary_page: &mut temporary_page::TemporaryPage, // new
                   f: F)
        where F: FnOnce(&mut Mapper)
    {
        use x86::shared::{control_regs, tlb};
        let flush_tlb = || unsafe { tlb::flush_all() };

        {
            let backup = Frame::containing_address(unsafe { control_regs::cr3() } as usize);

            // map temporary_page to current p4 table
            let p4_table = temporary_page.map_table_frame(backup.clone(), self);

            // overwrite recursive mapping
            self.p4_mut()[511].set(table.p4_frame.clone(), PRESENT | WRITABLE);
            flush_tlb();

            // execute f in the new context
            f(self);

            // restore recursive mapping to original p4 table
            p4_table[511].set(backup, PRESENT | WRITABLE);
            flush_tlb();
        }

        temporary_page.unmap(self);
    }

    pub fn switch(&mut self, new_table: InactivePageTable) -> InactivePageTable {
        use x86::shared::control_regs;

        let old_table = InactivePageTable {
            p4_frame: Frame::containing_address(unsafe { control_regs::cr3() } as usize),
        };
        unsafe {
            control_regs::cr3_write(new_table.p4_frame.start_address());
        }
        old_table
    }
}

pub struct InactivePageTable {
    p4_frame: Frame,
}

impl InactivePageTable {
    pub fn new(frame: Frame,
                active_table: &mut ActivePageTable,
                temporary_page: &mut TemporaryPage)
                -> InactivePageTable
    {
        {
            let table = temporary_page.map_table_frame(frame.clone(), active_table);

            // zero the page table
            table.zero();

            // set up recursive mapping for the table
            table[511].set(frame.clone(), PRESENT | WRITABLE);
        }
        temporary_page.unmap(active_table);

        InactivePageTable { p4_frame: frame }
    }
}

pub fn test_paging<A>(allocator: &mut A)
    where A: FrameAllocator
{
    let mut page_table = unsafe { ActivePageTable::new() };

    // test translate
    println!("Some = {:?}", page_table.translate(0));
    println!("Some = {:?}", page_table.translate(4096)); // second P1 entry
    println!("Some = {:?}", page_table.translate(512 * 4096)); // second P2 entry
    println!("Some = {:?}", page_table.translate(300 * 512 * 4096)); // 300th P2 entry
    println!("None = {:?}", page_table.translate(512 * 512 * 4096)); // second P3 entry
    println!("Some = {:?}", page_table.translate(512 * 512 * 4096 - 1)); // last mapped byte

    // test map_to
    let addr = 42 * 512 * 512 * 4096; // 42th P3 entry
    let page = Page::containing_address(addr);
    let frame = allocator.allocate_frame().expect("no more frames");
    println!("None = {:?}, map to {:?}",
             page_table.translate(addr),
             frame);
    page_table.map_to(page, frame, EntryFlags::empty(), allocator);
    println!("Some = {:?}", page_table.translate(addr));
    println!("next free frame: {:?}", allocator.allocate_frame());

    // test unmap
    println!("{:#x}",
             unsafe { *(Page::containing_address(addr).start_address() as *const u64) });
    page_table.unmap(Page::containing_address(addr), allocator);
    println!("None = {:?}", page_table.translate(addr));
}
