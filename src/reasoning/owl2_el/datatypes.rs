//! The OWL 2 EL datatype map (OWL 2 Profiles §2.2.1): nineteen datatypes,
//! no facets, value spaces and value equality.
//!
//! EL and QL share one datatype map, [`crate::reasoning::datatypes`]; this
//! module re-exports what the EL engine uses from it. EL keeps only
//! datatypes whose value spaces intersect either not at all or infinitely, so
//! data reasoning reduces to three facts per value: which datatypes contain
//! it, which datatypes are disjoint, and when two literals denote the same
//! value. The EL engine turns the datatypes into concepts and each distinct
//! value into a nominal.

pub(crate) use crate::reasoning::datatypes::{datatypes_of, value_of, Dt, Value};
