#![cfg(feature = "dynamic_query")]
use crate::component::ComponentId;
use alloc::vec::Vec;
use smallvec::SmallVec;
use super::FilteredAccess;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct VarId(pub u32);

#[derive(Copy, Clone, Debug)]
pub enum TermVar { This, Var(VarId) }

#[derive(Clone, Debug)]
pub enum Constraint {
    With { var: TermVar, component: ComponentId, write: bool },
    Without { var: TermVar, component: ComponentId },
    Relation { rel: ComponentId, from: TermVar, to: TermVar },
}

#[derive(Clone, Debug)]
pub struct DynamicPlan {
    pub vars: SmallVec<[TermVar; 3]>,
    pub constraints: Vec<Constraint>,
    pub filtered_access: FilteredAccess,
}

impl DynamicPlan {
    pub fn new() -> Self {
        let mut vars = SmallVec::new();
        vars.push(TermVar::This);
        Self { vars, constraints: Vec::new(), filtered_access: FilteredAccess::default() }
    }
    pub fn var(&mut self) -> VarId {
        let id = VarId(self.vars.len() as u32);
        self.vars.push(TermVar::Var(id));
        id
    }
}


