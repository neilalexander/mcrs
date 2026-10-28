use core::convert::Infallible;

use embedded_hal::{
    delay::DelayNs as BlockingDelayNs,
    digital::OutputPin,
    spi::{ErrorType as BlockingErrorType, SpiBus as BlockingSpiBus},
};
use embedded_hal_async::spi::{ErrorType as AsyncErrorType, Operation, SpiDevice};

pub struct BlockingExclusiveSpiDevice<BUS, CS, DLY> {
    bus: BUS,
    cs: CS,
    delay: DLY,
}

impl<BUS, CS, DLY> BlockingExclusiveSpiDevice<BUS, CS, DLY>
where
    CS: OutputPin<Error = Infallible>,
{
    pub fn new(bus: BUS, mut cs: CS, delay: DLY) -> Self {
        let _ = cs.set_high();
        Self { bus, cs, delay }
    }
}

impl<BUS, CS, DLY> AsyncErrorType for BlockingExclusiveSpiDevice<BUS, CS, DLY>
where
    BUS: BlockingErrorType,
{
    type Error = BUS::Error;
}

impl<BUS, CS, DLY> SpiDevice<u8> for BlockingExclusiveSpiDevice<BUS, CS, DLY>
where
    BUS: BlockingSpiBus<u8>,
    CS: OutputPin<Error = Infallible>,
    DLY: BlockingDelayNs,
{
    async fn transaction(
        &mut self,
        operations: &mut [Operation<'_, u8>],
    ) -> Result<(), Self::Error> {
        let _ = self.cs.set_low();

        let result = self.transaction_inner(operations);
        let flush_result = self.bus.flush();

        let _ = self.cs.set_high();

        result?;
        flush_result
    }
}

impl<BUS, CS, DLY> BlockingExclusiveSpiDevice<BUS, CS, DLY>
where
    BUS: BlockingSpiBus<u8>,
    CS: OutputPin<Error = Infallible>,
    DLY: BlockingDelayNs,
{
    fn transaction_inner(
        &mut self,
        operations: &mut [Operation<'_, u8>],
    ) -> Result<(), BUS::Error> {
        for operation in operations {
            match operation {
                Operation::Read(buffer) => self.bus.read(buffer)?,
                Operation::Write(buffer) => self.bus.write(buffer)?,
                Operation::Transfer(read, write) => self.bus.transfer(read, write)?,
                Operation::TransferInPlace(buffer) => self.bus.transfer_in_place(buffer)?,
                Operation::DelayNs(ns) => self.delay.delay_ns(*ns),
            }
        }

        Ok(())
    }
}
