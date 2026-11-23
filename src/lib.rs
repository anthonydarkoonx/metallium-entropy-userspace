//! The entropy driver (role 21): virtio-rng over PCI, driven from user space. It reads 32 bytes from the device, hands them to the
//! kernel with `ADD_ENTROPY` (which folds them into the generator that lays out address spaces and serves `GETRANDOM`), and does it
//! again once a minute, so the generator is reseeded for as long as the machine runs.
//!
//! The kernel used to read the device itself, once, while it probed the bus, and shut it down again. Now it only finds the device
//! (`kernel/src/devices.rs`), confines it behind the IOMMU to one page, and starts this task last of all, so every other task keeps its
//! id. Tasks made before the first delivery are laid out from RNDR, the seed file and the counter alone; on a machine with none of those
//! the generator is not "strong" (`GETRANDOM` answers `E_NOSYS`) until this driver has delivered.
//!
//! The user image starts it with `auto_maps()` already called (the kernel refuses a boot-time service any address it did not choose, `mem_map`).
//!
//! Capabilities (slots from the kernel's driver table): 1 the device's registers, 2 the page holding the queue and the buffer (the only
//! memory the device can reach), 3 a read-only copy of its PCI configuration space. No interrupt: the used ring is polled.
//!
//! It never exits: a machine whose generator takes no outside entropy (the layout debugging modes) makes `ADD_ENTROPY` answer
//! `E_NOSYS`, and the driver then just sleeps.
#![no_std]

use metallium_driver_rt::virtio;
use metallium_driver_rt::*;

const MMIO_VA: usize = 0x8020_0000;
const QUEUE_VA: usize = 0x8021_0000;
const CFG_VA: usize = 0x8022_0000;
/// The virtio-rng device (modern: 0x1040 + device type 4).
const VIRTIO_RNG: u16 = 0x1044;
/// The buffer the device fills: after the descriptor table, the available ring and the used ring.
const DATA: usize = 0x400;
const BYTES: usize = 32;
/// A device that does not answer in this many milliseconds is given up on (until the next round).
const ANSWER_MS: u64 = 50;
/// Ticks (100 per second) between two deliveries.
const RESEED_TICKS: u64 = 100 * 60;
/// Ticks to wait before trying again after a failure.
const RETRY_TICKS: u64 = 100 * 5;

struct Dev {
    q: virtio::Queue,
    queue_va: usize,
    queue_pa: u64,
}

fn init() -> Result<Dev, &'static str> {
    if mem_map(1, MMIO_VA, true) != 0 || mem_map(2, QUEUE_VA, true) != 0 || mem_map(3, CFG_VA, false) != 0 {
        return Err("mapping failed");
    }
    let (mmio_va, queue_va, cfg_va) = (va(MMIO_VA), va(QUEUE_VA), va(CFG_VA)); // (where the kernel put them)
    let (s1, _, bar_pages, _) = cap_info(1);
    let (s2, queue_pa, _, _) = cap_info(2);
    if s1 != 0 || s2 != 0 {
        return Err("cap_info failed");
    }
    // SAFETY: the BAR and config-space mappings the kernel gave this driver (`bar_pages` pages at MMIO_VA / CFG_VA, per its facts).
    let pci = unsafe { virtio::Pci::open(mmio_va, bar_pages as usize * 4096, cfg_va, VIRTIO_RNG) }?;
    pci.start()?;
    // SAFETY: `queue_va` is the page just mapped (checked) read-write; the device has not been given a queue yet. (A crashed predecessor's ring state is forgotten.)
    unsafe { core::ptr::write_bytes(queue_va as *mut u8, 0, 4096) };
    let q = pci.queue(0, 4, queue_va, queue_pa)?; // queue 0 is the request queue
    pci.driver_ok();
    Ok(Dev { q, queue_va, queue_pa })
}

impl Dev {
    /// Ask the device for `BYTES` bytes of entropy.
    fn read(&mut self) -> Option<[u8; BYTES]> {
        let buf = virtio::Buf { pa: self.queue_pa + DATA as u64, va: self.queue_va + DATA, len: BYTES as u32, dev_writes: true };
        self.q.submit(&[buf]);
        let mut waited = 0;
        while !self.q.done() {
            if waited >= ANSWER_MS {
                return None;
            }
            sleep_us(1000);
            waited += 1;
        }
        let mut out = [0u8; BYTES];
        for (i, b) in out.iter_mut().enumerate() {
            // SAFETY: `DATA + i` with `i < BYTES` lies inside the queue page this task has mapped read-write; volatile because the device wrote it (`done` flushed the cache lines).
            *b = unsafe { read_volatile((self.queue_va + DATA + i) as *const u8) };
        }
        // (the device writes the used length after the data: a device that wrote nothing gave nothing)
        out.iter().any(|&b| b != 0).then_some(out)
    }
}

fn idle() -> ! {
    loop {
        sleep(RESEED_TICKS);
    }
}

/// The entropy driver's task (role 21).
pub fn rng_driver() -> ! {
    let mut dev = match init() {
        Ok(d) => d,
        Err(e) => {
            println!("[rand] virtio-rng present but could not be started: {e}");
            idle()
        }
    };
    let mut first = true;
    loop {
        let mut bytes = match dev.read() {
            Some(b) => b,
            None => {
                if first {
                    println!("[rand] virtio-rng present but gave no entropy");
                }
                first = false;
                sleep(RETRY_TICKS);
                continue;
            }
        };
        let r = add_entropy(&bytes);
        bytes.fill(0);
        match r {
            0 => {
                if first {
                    println!("[rand] virtio-rng: {BYTES} bytes folded into the layout generator");
                }
            }
            E_NOSYS => {
                println!("[rand] virtio-rng not read: layouts are meant to follow from the seed alone (or are not randomised)");
                idle()
            }
            e => {
                println!("[rand] ADD_ENTROPY refused ({e})");
                idle()
            }
        }
        first = false;
        sleep(RESEED_TICKS);
    }
}
