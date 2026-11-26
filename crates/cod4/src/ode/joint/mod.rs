//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! Joints: IW's `dxJoint` has no vtable, an int type at +0x14 instead,
//! dispatched by `getInfo1` (0x5b5b30) and `getInfo2` (0x5b5bd0);
//! cod4-player-anims.md 10.4.

pub mod amotor;
pub mod ball;
pub mod contact;
pub mod hinge;
pub mod limot;

use super::body::Body;
use super::quickstep::Row;
use super::world::{BodyId, Params};

/// A joint's row counts (`dxJoint::Info1`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Info1 {
    pub m: usize,
    pub nub: usize,
}

/// What a joint fills for its rows (`dxJoint::Info2`): IW writes the
/// Jacobian, `lo` and `hi` into the row records, `c`, `cfm` and `findex`
/// into their own arrays.
pub struct Info2<'a> {
    pub fps: f32,
    pub erp: f32,
    pub rows: &'a mut [Row],
    pub c: &'a mut [f32],
    pub cfm: &'a mut [f32],
    pub findex: &'a mut [i32],
    /// Torques a limit motor pressing on a stop adds (`dBodyAddTorque`), by
    /// end, in order.
    pub torque: Vec<(usize, super::math::V3)>,
}

#[derive(Clone, Debug)]
pub enum Kind {
    Ball(ball::Ball),
    Hinge(hinge::Hinge),
    Contact(contact::Contact),
    AMotor(amotor::AMotor),
}

impl Kind {
    /// The int at joint +0x14 the dispatchers switch on.
    pub fn type_id(&self) -> i32 {
        match self {
            Kind::Ball(_) => 1,
            Kind::Hinge(_) => 2,
            Kind::Contact(_) => 4,
            Kind::AMotor(_) => 8,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Joint {
    pub kind: Kind,
    /// `node[0].body`, `node[1].body`: a world end is always the second.
    pub body: [Option<BodyId>; 2],
    /// `dJOINT_REVERSE`: attached with the world first.
    pub reverse: bool,
    pub(crate) tag: i32,
    /// The ends' indices in the island being stepped (+0x24, +0x34), -1
    /// for the world.
    pub(crate) index: [i32; 2],
}

impl Joint {
    pub fn new(kind: Kind) -> Joint {
        Joint {
            kind,
            body: [None, None],
            reverse: false,
            tag: 0,
            index: [-1, -1],
        }
    }

    pub(crate) fn info1(&mut self, bodies: &[Option<Body>]) -> Info1 {
        let b = self.ends(bodies);
        match &mut self.kind {
            Kind::Ball(_) => Info1 { m: 3, nub: 3 },
            Kind::Hinge(j) => j.info1(b),
            Kind::Contact(j) => j.info1(),
            Kind::AMotor(j) => j.info1(b),
        }
    }

    pub(crate) fn info2(&self, bodies: &[Option<Body>], params: &Params, info: &mut Info2) {
        let b = self.ends(bodies);
        match &self.kind {
            Kind::Ball(j) => j.info2(b, info),
            Kind::Hinge(j) => j.info2(b, info),
            Kind::Contact(j) => j.info2(b, self.reverse, params, info),
            Kind::AMotor(j) => j.info2(b, info),
        }
    }

    /// The two ends' bodies.
    pub(crate) fn ends<'b>(&self, bodies: &'b [Option<Body>]) -> [Option<&'b Body>; 2] {
        self.body
            .map(|b| b.map(|b| bodies[b.0 as usize].as_ref().expect("live body")))
    }
}
