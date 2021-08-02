use byteorder::{ByteOrder, LittleEndian};
use ckb_vm::{
    decoder::build_decoder, machine::asm::AsmMachine, CoreMachine, Memory, SupportMachine,
    RISCV_GENERAL_REGISTER_NUMBER,
};
use gdb_remote_protocol::{
    Breakpoint, Error, Handler, MemoryRegion, ProcessType, StopReason, ThreadId, VCont,
    VContFeature,
};
use std::borrow::Cow;
use std::cell::RefCell;

fn format_register_value(v: u64) -> Vec<u8> {
    let mut buf = [0u8; 8];
    LittleEndian::write_u64(&mut buf, v);
    buf.to_vec()
}

pub struct GdbHandler<'a> {
    machine: RefCell<AsmMachine<'a>>,
    breakpoints: RefCell<Vec<Breakpoint>>,
}

impl<'a> GdbHandler<'a> {
    fn at_breakpoint(&self) -> bool {
        let pc = *self.machine.borrow().machine.pc();
        self.breakpoints.borrow().iter().any(|b| b.addr == pc)
    }

    pub fn new(machine: AsmMachine<'a>) -> Self {
        GdbHandler {
            machine: RefCell::new(machine),
            breakpoints: RefCell::new(vec![]),
        }
    }
}

impl<'a> Handler for GdbHandler<'a> {
    fn attached(&self, _pid: Option<u64>) -> Result<ProcessType, Error> {
        Ok(ProcessType::Created)
    }

    fn halt_reason(&self) -> Result<StopReason, Error> {
        // SIGINT
        Ok(StopReason::Signal(2))
    }

    fn read_general_registers(&self) -> Result<Vec<u8>, Error> {
        let registers: Vec<Vec<u8>> = self
            .machine
            .borrow()
            .machine
            .registers()
            .iter()
            .map(|v| format_register_value(*v))
            .collect();
        Ok(registers.concat())
    }

    fn read_register(&self, register: u64) -> Result<Vec<u8>, Error> {
        let register = register as usize;
        if register < RISCV_GENERAL_REGISTER_NUMBER {
            Ok(format_register_value(
                self.machine.borrow().machine.registers()[register],
            ))
        } else if register == RISCV_GENERAL_REGISTER_NUMBER {
            Ok(format_register_value(*self.machine.borrow().machine.pc()))
        } else {
            Err(Error::Error(1))
        }
    }

    fn write_register(&self, register: u64, contents: &[u8]) -> Result<(), Error> {
        let mut buffer = [0u8; 8];
        if contents.len() > 8 {
            error!("Register value too large!");
            return Err(Error::Error(2));
        }
        buffer[0..contents.len()].copy_from_slice(contents);
        let value = LittleEndian::read_u64(&buffer[..]);
        let register = register as usize;
        if register < RISCV_GENERAL_REGISTER_NUMBER {
            self.machine
                .borrow_mut()
                .machine
                .set_register(register, value);
            Ok(())
        } else if register == RISCV_GENERAL_REGISTER_NUMBER {
            self.machine.borrow_mut().machine.update_pc(value);
            self.machine.borrow_mut().machine.commit_pc();
            Ok(())
        } else {
            Err(Error::Error(2))
        }
    }

    fn read_memory(&self, region: MemoryRegion) -> Result<Vec<u8>, Error> {
        let mut values = vec![];
        for address in region.address..(region.address + region.length) {
            let value = self
                .machine
                .borrow_mut()
                .machine
                .memory_mut()
                .load8(&address)
                .map_err(|e| {
                    error!("Error reading memory address {:x}: {:?}", address, e);
                    Error::Error(3)
                })?;
            values.push(value as u8);
        }
        Ok(values)
    }

    fn write_memory(&self, address: u64, bytes: &[u8]) -> Result<(), Error> {
        self.machine
            .borrow_mut()
            .machine
            .memory_mut()
            .store_bytes(address, bytes)
            .map_err(|e| {
                error!("Error writing memory address {:x}: {:?}", address, e);
                Error::Error(4)
            })?;
        Ok(())
    }

    fn query_supported_vcont(&self) -> Result<Cow<'static, [VContFeature]>, Error> {
        // Even though we won't support all of vCont features, gdb feature
        // detection only work when we include all of them. The other solution
        // is to use the plain old s or c, but the RSP parser we are using here
        // doesn't support them yet.
        Ok(Cow::from(
            &[
                VContFeature::Continue,
                VContFeature::ContinueWithSignal,
                VContFeature::Step,
                VContFeature::StepWithSignal,
                VContFeature::Stop,
                VContFeature::RangeStep,
            ][..],
        ))
    }

    fn vcont(&self, request: Vec<(VCont, Option<ThreadId>)>) -> Result<StopReason, Error> {
        let mut decoder = build_decoder::<u64>(self.machine.borrow().machine.isa());
        let (vcont, _thread_id) = &request[0];
        match vcont {
            VCont::Continue => {
                self.machine
                    .borrow_mut()
                    .machine
                    .step(&mut decoder)
                    .expect("VM error");
                while (!self.at_breakpoint()) && self.machine.borrow().machine.running() {
                    self.machine
                        .borrow_mut()
                        .machine
                        .step(&mut decoder)
                        .expect("VM error");
                }
            }
            VCont::Step => {
                if self.machine.borrow().machine.running() {
                    self.machine
                        .borrow_mut()
                        .machine
                        .step(&mut decoder)
                        .expect("VM error");
                }
            }
            VCont::RangeStep(range) => {
                self.machine
                    .borrow_mut()
                    .machine
                    .step(&mut decoder)
                    .expect("VM error");
                while self.machine.borrow().machine.pc() >= &range.start
                    && self.machine.borrow().machine.pc() < &range.end
                    && (!self.at_breakpoint())
                    && self.machine.borrow().machine.running()
                {
                    self.machine
                        .borrow_mut()
                        .machine
                        .step(&mut decoder)
                        .expect("VM error");
                }
            }
            v => {
                debug!("Unspported vcont type: {:?}", v);
                return Err(Error::Error(5));
            }
        }
        if self.machine.borrow().machine.running() {
            // SIGTRAP
            Ok(StopReason::Signal(5))
        } else {
            Ok(StopReason::Exited(
                0,
                self.machine.borrow().machine.exit_code() as u8,
            ))
        }
    }

    fn insert_software_breakpoint(&self, breakpoint: Breakpoint) -> Result<(), Error> {
        self.breakpoints.borrow_mut().push(breakpoint);
        Ok(())
    }

    fn remove_software_breakpoint(&self, breakpoint: Breakpoint) -> Result<(), Error> {
        self.breakpoints.borrow_mut().retain(|b| b != &breakpoint);
        Ok(())
    }
}
