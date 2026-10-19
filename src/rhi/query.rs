#![allow(dead_code)]

use super::backend::RhiQueryPool;
use super::lifetime::{ObjectId, RetireQueue};
use super::sync::SyncPoint;
use std::sync::{Arc, Weak};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QueryType {
    Timestamp,
}

#[derive(Clone, Debug)]
pub struct QueryPoolDesc {
    pub name: String,
    pub ty: QueryType,
    pub count: u32,
}

pub struct QueryPool {
    pub(crate) id: ObjectId,
    pub(crate) desc: QueryPoolDesc,
    pub(crate) inner: Option<Arc<dyn RhiQueryPool>>,
    pub(crate) retire_queue: Weak<RetireQueue>,
}

#[derive(Clone, Copy, Debug)]
pub enum TimestampStage {
    Top,
    Bottom,
    Vertex,
    Fragment,
    Compute,
    Transfer,
}

#[derive(Clone, Copy, Debug)]
pub struct QueryResult {
    pub value: u64,
    pub available: bool,
}

#[derive(Clone, Debug)]
pub struct PassTiming {
    pub name: String,
    pub begin: u64,
    pub end: u64,
    pub nanoseconds_per_tick: f64,
    pub completion: SyncPoint,
}
