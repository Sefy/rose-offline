use bevy_ecs::prelude::Component;

use crate::data::NpcId;

#[derive(Component, Clone)]
pub struct Npc {
    pub id: NpcId,
    pub quest_index: u16,
}

impl Npc {
    pub fn new(id: NpcId, quest_index: u16) -> Self {
        Self { id, quest_index }
    }
}
