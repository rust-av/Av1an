mod akima;
mod catmull_rom;
mod cubic_polynomial;
mod fritsch_carlson;
mod linear;
mod natural_cubic_spline;
mod pchip;
mod quadratic;

pub use akima::akima;
pub use catmull_rom::catmull_rom;
pub use cubic_polynomial::cubic_polynomial;
pub use fritsch_carlson::fritsch_carlson;
pub use linear::linear;
pub use natural_cubic_spline::natural_cubic_spline;
pub use pchip::pchip;
pub use quadratic::quadratic;
