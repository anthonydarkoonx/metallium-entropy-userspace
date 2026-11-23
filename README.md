# metallium-entropy-userspace

The entropy driver of [Metallium-OS](https://github.com/anthonydarkoonx/metallium-os): a virtio-rng device driven from user space.
Licensed Apache-2.0 OR MIT.

The kernel used to read the device itself, once, while it probed the bus. Now it only finds the device, confines it behind the IOMMU
to one page, and starts this task (role 21) last of all, so every other task keeps its id. The driver reads 32 bytes and hands them
to the kernel with the `ADD_ENTROPY` system call, which folds them into the generator that lays out address spaces and serves
`GETRANDOM`. It does it again once a minute, so the generator is reseeded for as long as the machine runs.

| path | what |
|---|---|
| `src/lib.rs` | the driver: `rng_driver()`, the task's entry point. `no_std`, built on `metallium-driver-rt` (system calls, mappings, virtio) |

## What changes for the system
* **Boot-time layouts.** Tasks made before the first delivery are laid out from RNDR, the seed file and the counter alone (as before
  on any machine that has the first two). A machine with none of those has a generator that is not "strong" (`GETRANDOM` answers
  `E_NOSYS`) until this driver has delivered, which happens within the first moments of the boot. The kernel logs its usual
  `[rand] WARNING` for that case.
* **Debugging modes** (`noaslr`, `aslr-seed`, `aslr-log`): the kernel refuses outside entropy (`ADD_ENTROPY` answers `E_NOSYS`), the
  driver says so once and sleeps.
* **Who may call it.** Only a task of role 21. Any other caller gets `E_PERM`, so a program cannot feed the generator bytes it chose.
  (Bytes are folded into the generator's state: they cannot reduce what it already has.)
* **A compromised driver** can withhold entropy or contribute bytes it knows, but it cannot read the generator or lower its entropy.

## Building and testing
This repository and the OS repository must sit **side by side** (`~/Desktop/metallium-os` and `~/Desktop/metallium-entropy-userspace`,
say): the manifests refer to each other by relative path.

    cargo check                  # here: the driver compiles (on the host too; the system-call layer has host stand-ins)
    make entropy                 # in the OS repository: a normal boot folds the bytes in without a warning; the debugging modes skip them
    make test                    # in the OS repository: everything

The kernel side is in the OS repository: `kernel/src/devices.rs` (the device entry and its capabilities), `kernel/src/syscall.rs`
(`ADD_ENTROPY`) and `kernel/src/rand.rs`.
