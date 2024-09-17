//! Implements the router request pipeline
//!
//! A router pipeline takes requests from clients, in a variety of formats, transforms and
//! validates the requests in a number of stages and then issues fetch requests to a variety
//! of downstream systems.
//!
//!
//!

pub(crate) mod http;
mod json;
