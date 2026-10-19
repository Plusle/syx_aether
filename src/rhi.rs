#![allow(dead_code)]

//! Backend-neutral RHI declarations. Public handles intentionally have no methods yet.
//! Backend operations are declared by private traits; no backend is implemented here.

mod backend;
pub mod command;
pub mod descriptor;
pub mod device;
pub mod error;
mod lifetime;
pub mod memory;
pub mod pipeline;
pub mod query;
pub mod queue;
pub mod resource;
pub mod shader;
pub mod swapchain;
pub mod sync;
mod tracking;
pub mod types;

pub use command::*;
pub use descriptor::*;
pub use device::*;
pub use error::*;
pub use memory::*;
pub use pipeline::*;
pub use query::*;
pub use queue::*;
pub use resource::*;
pub use shader::*;
pub use swapchain::*;
pub use sync::*;
pub use types::*;
