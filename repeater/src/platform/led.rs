//! Board LEDs with logical on and off states.

use core::convert::Infallible;
pub use embedded_hal::digital::PinState;
use embedded_hal::digital::{Error, OutputPin};

/// An indicator whose electrical polarity is handled by the board.
pub trait Led {
    type Error: Error;

    fn set_lit(&mut self, lit: bool) -> Result<(), Self::Error>;
}

/// Keeps an LED lit until the guard goes out of scope.
pub struct LitLed<'a, L: Led>(&'a mut L);

impl<'a, L: Led> LitLed<'a, L> {
    pub fn new(led: &'a mut L) -> Result<Self, L::Error> {
        let guard = Self(led);
        guard.0.set_lit(true)?;
        Ok(guard)
    }
}

impl<L: Led> Drop for LitLed<'_, L> {
    fn drop(&mut self) {
        let _ = self.0.set_lit(false);
    }
}

/// An LED connected to a GPIO output.
pub struct GpioLed<P> {
    pin: P,
    active_level: PinState,
}

impl<P: OutputPin> GpioLed<P> {
    /// Sets up an LED in the off state. The active level is the GPIO level
    /// that lights the LED.
    pub fn new(pin: P, active_level: PinState) -> Result<Self, P::Error> {
        let mut led = Self { pin, active_level };
        led.set_lit(false)?;
        Ok(led)
    }
}

impl<P: OutputPin> Led for GpioLed<P> {
    type Error = P::Error;

    fn set_lit(&mut self, lit: bool) -> Result<(), Self::Error> {
        let level = if lit {
            self.active_level
        } else {
            !self.active_level
        };
        self.pin.set_state(level)
    }
}

/// An indicator for boards without an LED.
pub struct NoLed;

impl Led for NoLed {
    type Error = Infallible;

    fn set_lit(&mut self, _lit: bool) -> Result<(), Self::Error> {
        Ok(())
    }
}
