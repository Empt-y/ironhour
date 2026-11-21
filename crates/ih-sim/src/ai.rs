//! Movement subset of AIStateMachine/AIStates.cpp. Original numeric state IDs;
//! successful/failed actions return to idle, user orders interrupt old actions.
//! Combat, guard, transports, mood and temporary AI overrides remain pending.
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u32)]
pub enum AiState {
    #[default]
    Idle = 0,
    MoveTo = 1,
    FollowPath = 6,
    /// `AI_DEAD`.
    Dead = 13,
    FaceObject = 33,
    FacePosition = 34,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    #[default]
    None,
    Success,
    Failure,
    Interrupted,
}
#[derive(Clone, Debug, Default)]
pub struct AiMachine {
    pub state: AiState,
    pub entered_frame: u32,
    pub transitions: u32,
    pub outcome: Outcome,
    pub completed_orders: u32,
    pub failed_orders: u32,
    pub interrupted_orders: u32,
    pub remaining: VecDeque<[f32; 2]>,
    pub face_position: Option<[f32; 2]>,
    pub face_object: Option<u32>,
    /// Active move-out-of-the-way temporary state.
    pub move_out: Option<MoveOutOfWay>,
    /// `m_moveOutOfWay1`, `m_moveOutOfWay2`: the units most recently made way for.
    pub move_out_of_way: [Option<u32>; 2],
    /// `AIFollowPathState::m_retryCount` (10 per order, shared by its legs).
    pub retries: u32,
    /// `m_isAiDead` (`markAsDead`).
    pub dead: bool,
}
impl AiMachine {
    pub fn begin(&mut self, state: AiState, frame: u32) {
        self.move_out = None;
        self.move_out_of_way = [None; 2];
        if self.state != AiState::Idle {
            self.interrupted_orders += 1;
        }
        self.remaining.clear();
        self.face_position = None;
        self.face_object = None;
        self.retries = if state == AiState::FollowPath { 10 } else { 0 };
        self.state = state;
        self.entered_frame = frame;
        self.transitions += 1;
        self.outcome = Outcome::None;
    }
    pub fn finish(&mut self, outcome: Outcome, frame: u32) {
        match outcome {
            Outcome::Success => self.completed_orders += 1,
            Outcome::Failure => self.failed_orders += 1,
            Outcome::Interrupted => {
                if self.state != AiState::Idle {
                    self.interrupted_orders += 1;
                }
            }
            Outcome::None => {}
        }
        if self.state != AiState::Idle {
            self.transitions += 1;
            self.entered_frame = frame;
        }
        self.state = AiState::Idle;
        self.outcome = outcome;
        self.move_out = None;
        self.move_out_of_way = [None; 2];
        self.remaining.clear();
        self.face_position = None;
        self.face_object = None;
    }
    pub fn active(&self) -> bool {
        self.state != AiState::Idle || self.move_out.is_some()
    }
}

/// `AI_MOVE_OUT_OF_THE_WAY` as a temporary state (`StateMachine::setTemporaryState`):
/// the unit follows a short path off another unit's route for up to ten seconds,
/// then resumes its previous order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MoveOutOfWay {
    /// The frame the temporary state expires.
    pub until: u32,
    /// The target of the interrupted order, if it was moving.
    pub resume_target: Option<[f32; 2]>,
}
