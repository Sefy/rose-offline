use crossbeam_channel::Receiver;
use legion::{Resources, Schedule, World};
use log::debug;
use std::time::{Duration, Instant};

use crate::game::{
    messages::control::ControlMessage,
    resources::{
        ClientEntityList, ControlChannel, DeltaTime, GameData, LoginTokens, PendingDamageList,
        PendingQuestTriggerList, PendingXpList, ServerList, ServerMessages, WorldRates, WorldTime,
    },
    systems::*,
};

pub struct GameWorld {
    tick_rate_hz: u64,
    control_rx: Receiver<ControlMessage>,
}

impl GameWorld {
    pub fn new(control_rx: Receiver<ControlMessage>) -> Self {
        Self {
            tick_rate_hz: 30,
            control_rx,
        }
    }

    pub fn run(&mut self, game_data: GameData) {
        let mut world = World::default();

        let mut resources = Resources::default();
        resources.insert(ControlChannel::new(self.control_rx.clone()));
        resources.insert(ServerList::new());
        resources.insert(LoginTokens::new());
        resources.insert(ServerMessages::new());
        resources.insert(ClientEntityList::new(&game_data.zones));
        resources.insert(PendingDamageList::new());
        resources.insert(PendingQuestTriggerList::new());
        resources.insert(PendingXpList::new());
        resources.insert(WorldRates::new());
        resources.insert(WorldTime::new());
        resources.insert(game_data);

        let started_load = Instant::now();
        let mut startup_schedule = Schedule::builder()
            .add_system(startup_zones_system())
            .build();
        startup_schedule.execute(&mut world, &mut resources);
        debug!(
            "Time taken to populate game world: {:?}",
            started_load.elapsed()
        );

        let mut schedule = Schedule::builder()
            .add_system(world_time_system())
            .add_system(control_server_system())
            .add_system(login_server_authentication_system())
            .add_system(login_server_system())
            .add_system(world_server_authentication_system())
            .add_system(world_server_system())
            .add_system(game_server_authentication_system())
            .add_system(game_server_join_system())
            .add_system(game_server_main_system())
            .add_system(game_server_disconnect_handler_system())
            .add_system(monster_spawn_system())
            .add_system(npc_ai_system())
            .add_system(expire_time_system())
            .flush()
            .add_system(command_system())
            .flush()
            .add_system(update_position_system())
            .add_system(apply_damage_system())
            .add_system(quest_system())
            .flush()
            .add_system(apply_pending_xp_system())
            .flush()
            .add_system(client_entity_visibility_system())
            .add_system(server_messages_sender_system())
            .build();

        let min_tick_duration = Duration::from_millis(1000 / self.tick_rate_hz);
        let mut last_tick = std::time::Instant::now();

        let mut tick_counter = 0;
        let mut tick_counter_duration = Duration::from_secs(0);
        let mut tick_counter_last_print = std::time::Instant::now();

        loop {
            let current_tick = std::time::Instant::now();
            resources.insert(DeltaTime {
                delta: current_tick - last_tick,
                now: current_tick,
            });
            schedule.execute(&mut world, &mut resources);

            let now = std::time::Instant::now();
            let tick_duration = now - current_tick;

            tick_counter += 1;
            tick_counter_duration += tick_duration;

            if now - tick_counter_last_print > Duration::from_secs(60) {
                let average_tick_duration =
                    tick_counter_duration.as_secs_f64() / (tick_counter as f64);
                debug!(
                    "Average tick duration: {:?}",
                    Duration::from_secs_f64(average_tick_duration)
                );

                tick_counter = 0;
                tick_counter_duration = Duration::from_secs(0);
                tick_counter_last_print = now;
            }

            if tick_duration < min_tick_duration {
                std::thread::sleep(min_tick_duration - tick_duration);
            }
            last_tick = current_tick;
        }
    }
}
