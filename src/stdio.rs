use ckb_vm::{
    registers::{A0, A1, A7},
    Error, Memory, Register, SupportMachine, Syscalls,
};
use nix::sys::stat::fstat;
use std::mem::size_of;
use std::slice::from_raw_parts;

#[derive(Clone, Debug, Default)]
#[repr(C)]
struct AbiStat {
    dev: u64,
    ino: u64,
    mode: u32,
    nlink: i32,
    uid: u32,
    gid: u32,
    rdev: u64,
    __pad1: u64,
    size: i64,
    blksize: i32,
    __pad2: i32,
    blocks: i64,
    atime: i64,
    atime_nsec: i64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
    __unused4: i32,
    __unused5: i32,
}

pub struct Stdio {}

impl Stdio {
    fn fstat<Mac: SupportMachine>(&mut self, machine: &mut Mac) -> Result<(), Error> {
        let stat = match fstat(machine.registers()[A0].to_i32()) {
            Ok(stat) => stat,
            Err(e) => {
                println!("fstat error: {:?}", e);
                machine.set_register(A0, Mac::REG::from_i8(-1));
                return Ok(());
            }
        };
        let mut abi_stat = AbiStat::default();
        abi_stat.dev = stat.st_dev;
        abi_stat.ino = stat.st_ino;
        abi_stat.mode = stat.st_mode;
        abi_stat.nlink = stat.st_nlink as i32;
        abi_stat.uid = stat.st_uid;
        abi_stat.gid = stat.st_gid;
        abi_stat.rdev = stat.st_rdev;
        abi_stat.size = stat.st_size;
        abi_stat.blksize = stat.st_blksize as i32;
        abi_stat.blocks = stat.st_blocks;
        abi_stat.atime = stat.st_atime;
        abi_stat.atime_nsec = stat.st_atime_nsec;
        abi_stat.mtime = stat.st_mtime;
        abi_stat.mtime_nsec = stat.st_mtime_nsec;
        abi_stat.ctime = stat.st_ctime;
        abi_stat.ctime_nsec = stat.st_ctime_nsec;
        let len = size_of::<AbiStat>();
        let b: &[u8] = unsafe { from_raw_parts(&abi_stat as *const AbiStat as *const u8, len) };
        let addr = machine.registers()[A1].to_u64();
        machine.memory_mut().store_bytes(addr, b)?;
        machine.set_register(A0, Mac::REG::zero());
        Ok(())
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for Stdio {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), Error> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, Error> {
        match machine.registers()[A7].to_u64() {
            80 => self.fstat(machine)?,
            _ => return Ok(false),
        };
        Ok(true)
    }
}
