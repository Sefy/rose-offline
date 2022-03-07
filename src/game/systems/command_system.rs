use std::time::Duration;

use bevy_ecs::prelude::{Commands, Entity, EventWriter, Mut, Query, Res, ResMut};
use nalgebra::Point3;

use crate::{
    data::{Item, ItemClass, SkillActionMode, StackableSlotBehaviour},
    game::{
        bundles::client_entity_leave_zone,
        components::{
            AbilityValues, AmmoIndex, ClientEntity, ClientEntitySector, ClientEntityType, Command,
            CommandAttack, CommandCastSkill, CommandCastSkillTarget, CommandData, CommandEmote,
            CommandMove, CommandPickupItemDrop, CommandSit, CommandStop, Destination, DroppedItem,
            Equipment, EquipmentIndex, GameClient, HealthPoints, Inventory, ItemDrop, ItemSlot,
            MotionData, MoveMode, MoveSpeed, NextCommand, Npc, Owner, PersonalStore, Position,
            Target,
        },
        events::{DamageEvent, SkillEvent, SkillEventTarget},
        messages::server::{
            self, PickupItemDropContent, PickupItemDropError, PickupItemDropResult, ServerMessage,
        },
        resources::{ClientEntityList, GameData, ServerMessages, ServerTime},
    },
};

const NPC_MOVE_TO_DISTANCE: f32 = 250.0;
const CHARACTER_MOVE_TO_DISTANCE: f32 = 1000.0;
const DROPPED_ITEM_MOVE_TO_DISTANCE: f32 = 150.0;
const DROPPED_ITEM_PICKUP_DISTANCE: f32 = 200.0;

fn command_stop(
    commands: &mut Commands,
    command: &mut Command,
    entity: Entity,
    client_entity: &ClientEntity,
    position: &Position,
    server_messages: Option<&mut ServerMessages>,
) {
    // Remove all components associated with other actions
    commands
        .entity(entity)
        .remove::<Destination>()
        .remove::<Target>();

    if let Some(server_messages) = server_messages {
        server_messages.send_entity_message(
            client_entity,
            ServerMessage::StopMoveEntity(server::StopMoveEntity {
                entity_id: client_entity.id,
                x: position.position.x,
                y: position.position.y,
                z: position.position.z as u16,
            }),
        );
    }

    *command = Command::with_stop();
}

fn is_valid_move_target<'a>(
    position: &Position,
    target_entity: Entity,
    move_target_query: &'a Query<(&ClientEntity, &Position)>,
) -> Option<(&'a ClientEntity, &'a Position)> {
    if let Ok((target_client_entity, target_position)) = move_target_query.get(target_entity) {
        if target_position.zone_id == position.zone_id {
            return Some((target_client_entity, target_position));
        }
    }

    None
}

fn is_valid_attack_target<'a>(
    position: &Position,
    target_entity: Entity,
    attack_target_query: &'a Query<(&ClientEntity, &Position, &AbilityValues, &HealthPoints)>,
) -> Option<(&'a ClientEntity, &'a Position, &'a AbilityValues)> {
    // TODO: Check Team
    if let Ok((target_client_entity, target_position, target_ability_values, target_health)) =
        attack_target_query.get(target_entity)
    {
        if target_position.zone_id == position.zone_id && target_health.hp > 0 {
            return Some((target_client_entity, target_position, target_ability_values));
        }
    }

    None
}

fn is_valid_skill_target<'a>(
    position: &Position,
    target_entity: Entity,
    attack_target_query: &'a Query<(&ClientEntity, &Position, &AbilityValues, &HealthPoints)>,
) -> Option<(&'a ClientEntity, &'a Position, &'a AbilityValues)> {
    // TODO: Check Team
    if let Ok((target_client_entity, target_position, target_ability_values, _target_health)) =
        attack_target_query.get(target_entity)
    {
        if target_position.zone_id == position.zone_id {
            return Some((target_client_entity, target_position, target_ability_values));
        }
    }

    None
}

fn is_valid_pickup_target<'a>(
    position: &Position,
    target_entity: Entity,
    query: &'a mut Query<(
        &ClientEntity,
        &ClientEntitySector,
        &Position,
        &mut ItemDrop,
        Option<&Owner>,
    )>,
) -> Option<(
    &'a ClientEntity,
    &'a ClientEntitySector,
    &'a Position,
    Mut<'a, ItemDrop>,
    Option<&'a Owner>,
)> {
    if let Ok((
        target_client_entity,
        target_client_entity_sector,
        target_position,
        target_item_drop,
        target_owner,
    )) = query.get_mut(target_entity)
    {
        // Check distance to target
        let distance = (position.position.xy() - target_position.position.xy()).magnitude();
        if position.zone_id == target_position.zone_id && distance <= DROPPED_ITEM_PICKUP_DISTANCE {
            return Some((
                target_client_entity,
                target_client_entity_sector,
                target_position,
                target_item_drop,
                target_owner,
            ));
        }
    }

    None
}

pub fn command_system(
    mut commands: Commands,
    mut query: Query<(
        Entity,
        &ClientEntity,
        &Position,
        &MotionData,
        &AbilityValues,
        &MoveMode,
        &mut Command,
        &mut NextCommand,
        Option<&GameClient>,
        Option<&mut Equipment>,
        Option<&mut Inventory>,
        Option<&Npc>,
        Option<&PersonalStore>,
    )>,
    move_target_query: Query<(&ClientEntity, &Position)>,
    attack_target_query: Query<(&ClientEntity, &Position, &AbilityValues, &HealthPoints)>,
    mut pickup_item_drop_target_query: Query<(
        &ClientEntity,
        &ClientEntitySector,
        &Position,
        &mut ItemDrop,
        Option<&Owner>,
    )>,
    server_time: Res<ServerTime>,
    mut client_entity_list: ResMut<ClientEntityList>,
    mut damage_events: EventWriter<DamageEvent>,
    mut skill_events: EventWriter<SkillEvent>,
    mut server_messages: ResMut<ServerMessages>,
    game_data: Res<GameData>,
) {
    query.for_each_mut(
        |(
            entity,
            client_entity,
            position,
            motion_data,
            ability_values,
            move_mode,
            mut command,
            mut next_command,
            game_client,
            equipment,
            inventory,
            npc,
            personal_store,
        )| {
            if !next_command.has_sent_server_message && next_command.command.is_some() {
                // Send any server message required for update client next command
                match next_command.command.as_mut().unwrap() {
                    CommandData::Die(_) => {
                        panic!("Next command should never be set to die, set current command")
                    }
                    CommandData::Sit(_) => {}
                    CommandData::Stop(_) => {}
                    CommandData::PersonalStore => {}
                    CommandData::PickupItemDrop(_) => {}
                    CommandData::Emote(_) => {}
                    CommandData::Move(CommandMove {
                        destination,
                        target,
                        move_mode: command_move_mode,
                    }) => {
                        let mut target_entity_id = None;
                        if let Some(target_entity) = target {
                            if let Some((target_client_entity, target_position)) =
                                is_valid_move_target(position, *target_entity, &move_target_query)
                            {
                                *destination = target_position.position;
                                target_entity_id = Some(target_client_entity.id);
                            } else {
                                *target = None;
                            }
                        }

                        let distance = (destination.xy() - position.position.xy()).magnitude();
                        server_messages.send_entity_message(
                            client_entity,
                            ServerMessage::MoveEntity(server::MoveEntity {
                                entity_id: client_entity.id,
                                target_entity_id,
                                distance: distance as u16,
                                x: destination.x,
                                y: destination.y,
                                z: destination.z as u16,
                                move_mode: *command_move_mode,
                            }),
                        );
                    }
                    CommandData::Attack(CommandAttack {
                        target: target_entity,
                    }) => {
                        if let Some((target_client_entity, target_position, ..)) =
                            is_valid_attack_target(position, *target_entity, &attack_target_query)
                        {
                            let distance = (target_position.position.xy() - position.position.xy())
                                .magnitude();

                            server_messages.send_entity_message(
                                client_entity,
                                ServerMessage::AttackEntity(server::AttackEntity {
                                    entity_id: client_entity.id,
                                    target_entity_id: target_client_entity.id,
                                    distance: distance as u16,
                                    x: target_position.position.x,
                                    y: target_position.position.y,
                                    z: target_position.position.z as u16,
                                }),
                            );
                        } else {
                            *next_command = NextCommand::with_stop(true);
                        }
                    }
                    CommandData::CastSkill(CommandCastSkill {
                        skill_id,
                        skill_target: None,
                        cast_motion_id,
                        ..
                    }) => {
                        server_messages.send_entity_message(
                            client_entity,
                            ServerMessage::CastSkillSelf(server::CastSkillSelf {
                                entity_id: client_entity.id,
                                skill_id: *skill_id,
                                cast_motion_id: *cast_motion_id,
                            }),
                        );
                    }
                    CommandData::CastSkill(CommandCastSkill {
                        skill_id,
                        skill_target: Some(CommandCastSkillTarget::Entity(target_entity)),
                        cast_motion_id,
                        ..
                    }) => {
                        if let Some((target_client_entity, target_position, ..)) =
                            is_valid_skill_target(position, *target_entity, &attack_target_query)
                        {
                            let distance = (target_position.position.xy() - position.position.xy())
                                .magnitude();

                            server_messages.send_entity_message(
                                client_entity,
                                ServerMessage::CastSkillTargetEntity(
                                    server::CastSkillTargetEntity {
                                        entity_id: client_entity.id,
                                        skill_id: *skill_id,
                                        target_entity_id: target_client_entity.id,
                                        target_distance: distance,
                                        target_position: target_position.position.xy(),
                                        cast_motion_id: *cast_motion_id,
                                    },
                                ),
                            );
                        } else {
                            *next_command = NextCommand::with_stop(true);
                        }
                    }
                    CommandData::CastSkill(CommandCastSkill {
                        skill_id,
                        skill_target: Some(CommandCastSkillTarget::Position(target_position)),
                        cast_motion_id,
                        ..
                    }) => {
                        server_messages.send_entity_message(
                            client_entity,
                            ServerMessage::CastSkillTargetPosition(
                                server::CastSkillTargetPosition {
                                    entity_id: client_entity.id,
                                    skill_id: *skill_id,
                                    target_position: *target_position,
                                    cast_motion_id: *cast_motion_id,
                                },
                            ),
                        );
                    }
                }

                next_command.has_sent_server_message = true;
            }

            command.duration += server_time.delta;

            let required_duration = match &mut command.command {
                CommandData::Attack(_) => {
                    let attack_speed =
                        i32::max(ability_values.get_attack_speed(), 30) as f32 / 100.0;
                    command
                        .required_duration
                        .map(|duration| duration.div_f32(attack_speed))
                }
                CommandData::Emote(_) => {
                    // Any command can interrupt an emote
                    if next_command.command.is_some() {
                        None
                    } else {
                        command.required_duration
                    }
                }
                _ => command.required_duration,
            };

            let command_motion_completed = required_duration.map_or_else(
                || true,
                |required_duration| command.duration >= required_duration,
            );

            if !command_motion_completed {
                // Current command still in animation
                return;
            }

            match command.command {
                CommandData::Die(_) => {
                    // We can't perform NextCommand if we are dead!
                    return;
                }
                CommandData::Sit(CommandSit::Sitting) => {
                    // When sitting animation is complete transition to Sit
                    *command = Command::with_sit();
                }
                _ => {}
            }

            if next_command.command.is_none() {
                // If we have completed current command, and there is no next command, then clear current.
                // This does not apply for some commands which must be manually completed, such as Sit
                // where you need to stand after.
                if command_motion_completed && !command.command.is_manual_complete() {
                    *command = Command::default();
                }

                // Nothing to do when there is no next command
                return;
            }

            if matches!(command.command, CommandData::Sit(CommandSit::Sit)) {
                // If current command is sit, we must stand before performing NextCommand
                let duration = motion_data
                    .get_sit_standing()
                    .map(|motion_data| motion_data.duration)
                    .unwrap_or_else(|| Duration::from_secs(0));

                *command = Command::with_standing(duration);

                server_messages
                    .send_entity_message(client_entity, ServerMessage::SitToggle(client_entity.id));
                return;
            }

            match next_command.command.as_mut().unwrap() {
                &mut CommandData::Stop(CommandStop { send_message }) => {
                    command_stop(
                        &mut commands,
                        &mut command,
                        entity,
                        client_entity,
                        position,
                        if send_message {
                            Some(&mut server_messages)
                        } else {
                            None
                        },
                    );
                    *next_command = NextCommand::default();
                }
                CommandData::Move(CommandMove {
                    destination,
                    target,
                    move_mode: command_move_mode,
                }) => {
                    let mut entity_commands = commands.entity(entity);

                    if let Some(target_entity) = *target {
                        if let Some((target_client_entity, target_position)) =
                            is_valid_move_target(position, target_entity, &move_target_query)
                        {
                            let required_distance = match target_client_entity.entity_type {
                                ClientEntityType::Character => Some(CHARACTER_MOVE_TO_DISTANCE),
                                ClientEntityType::Npc => Some(NPC_MOVE_TO_DISTANCE),
                                ClientEntityType::ItemDrop => Some(DROPPED_ITEM_MOVE_TO_DISTANCE),
                                _ => None,
                            };

                            if let Some(required_distance) = required_distance {
                                let offset = (target_position.position.xy()
                                    - position.position.xy())
                                .normalize()
                                    * required_distance;
                                destination.x = target_position.position.x - offset.x;
                                destination.y = target_position.position.y - offset.y;
                                destination.z = target_position.position.z;
                            } else {
                                *destination = target_position.position;
                            }
                        } else {
                            *target = None;
                            entity_commands.remove::<Target>();
                        }
                    }

                    match command_move_mode {
                        Some(MoveMode::Walk) => {
                            if !matches!(move_mode, MoveMode::Walk) {
                                entity_commands
                                    .insert(MoveMode::Walk)
                                    .insert(MoveSpeed::new(ability_values.get_walk_speed()));
                            }
                        }
                        Some(MoveMode::Run) => {
                            if !matches!(move_mode, MoveMode::Run) {
                                entity_commands
                                    .insert(MoveMode::Run)
                                    .insert(MoveSpeed::new(ability_values.get_run_speed()));
                            }
                        }
                        Some(MoveMode::Drive) => {
                            if !matches!(move_mode, MoveMode::Drive) {
                                entity_commands
                                    .insert(MoveMode::Drive)
                                    .insert(MoveSpeed::new(ability_values.get_drive_speed()));
                            }
                        }
                        None => {}
                    }

                    let distance = (destination.xy() - position.position.xy()).magnitude();
                    if distance < 0.1 {
                        *command = Command::with_stop();
                        entity_commands.remove::<Target>().remove::<Destination>();
                    } else {
                        *command = Command::with_move(*destination, *target, *command_move_mode);
                        entity_commands.insert(Destination::new(*destination));

                        if let Some(target_entity) = *target {
                            entity_commands.insert(Target::new(target_entity));
                        }
                    }
                }
                &mut CommandData::PickupItemDrop(CommandPickupItemDrop {
                    target: target_entity,
                }) => {
                    if let Some(mut inventory) = inventory {
                        if let Some((
                            target_client_entity,
                            target_client_entity_sector,
                            target_position,
                            mut target_item_drop,
                            target_owner,
                        )) = is_valid_pickup_target(
                            position,
                            target_entity,
                            &mut pickup_item_drop_target_query,
                        ) {
                            let result = if !target_owner
                                .map_or(true, |owner| owner.entity == entity)
                            {
                                // Not owner
                                Err(PickupItemDropError::NoPermission)
                            } else {
                                // Try add to inventory
                                match target_item_drop.item.take() {
                                    None => Err(PickupItemDropError::NotExist),
                                    Some(DroppedItem::Item(item)) => {
                                        match inventory.try_add_item(item) {
                                            Ok((slot, item)) => {
                                                Ok(PickupItemDropContent::Item(slot, item.clone()))
                                            }
                                            Err(item) => {
                                                target_item_drop.item =
                                                    Some(DroppedItem::Item(item));
                                                Err(PickupItemDropError::InventoryFull)
                                            }
                                        }
                                    }
                                    Some(DroppedItem::Money(money)) => {
                                        if inventory.try_add_money(money).is_ok() {
                                            Ok(PickupItemDropContent::Money(money))
                                        } else {
                                            target_item_drop.item = Some(DroppedItem::Money(money));
                                            Err(PickupItemDropError::InventoryFull)
                                        }
                                    }
                                }
                            };

                            if result.is_ok() {
                                // Delete picked up item
                                client_entity_leave_zone(
                                    &mut commands,
                                    &mut client_entity_list,
                                    target_entity,
                                    target_client_entity,
                                    target_client_entity_sector,
                                    target_position,
                                );
                                commands.entity(target_entity).despawn();

                                // Update our current command
                                let motion_duration =
                                    motion_data.get_pickup_item_drop().map_or_else(
                                        || Duration::from_secs(1),
                                        |motion| motion.duration,
                                    );

                                *command =
                                    Command::with_pickup_item_drop(target_entity, motion_duration);
                                commands
                                    .entity(entity)
                                    .remove::<Destination>()
                                    .remove::<Target>();
                            }

                            // Send message to client with result
                            if let Some(game_client) = game_client {
                                game_client
                                    .server_message_tx
                                    .send(ServerMessage::PickupItemDropResult(
                                        PickupItemDropResult {
                                            item_entity_id: target_client_entity.id,
                                            result,
                                        },
                                    ))
                                    .ok();
                            }
                        }

                        *next_command = NextCommand::default();
                    }
                }
                &mut CommandData::Attack(CommandAttack {
                    target: target_entity,
                }) => {
                    if let Some((_, target_position, target_ability_values)) =
                        is_valid_attack_target(position, target_entity, &attack_target_query)
                    {
                        let mut entity_commands = commands.entity(entity);
                        let distance =
                            (target_position.position.xy() - position.position.xy()).magnitude();

                        // Check if we are in attack range
                        let attack_range = ability_values.get_attack_range() as f32;
                        if distance < attack_range {
                            let (attack_duration, hit_count) = motion_data
                                .get_attack()
                                .as_ref()
                                .map(|attack_motion| {
                                    (attack_motion.duration, attack_motion.total_attack_frames)
                                })
                                .unwrap_or_else(|| (Duration::from_secs(1), 1));

                            // If the weapon uses ammo, we must consume the ammo
                            let mut cancel_attack = false;

                            if let Some(mut equipment) = equipment {
                                if let Some(weapon_data) = equipment
                                    .get_equipment_item(EquipmentIndex::WeaponRight)
                                    .and_then(|weapon_item| {
                                        game_data.items.get_base_item(weapon_item.item)
                                    })
                                {
                                    let ammo_index = match weapon_data.class {
                                        ItemClass::Bow | ItemClass::Crossbow => {
                                            Some(AmmoIndex::Arrow)
                                        }
                                        ItemClass::Gun | ItemClass::DualGuns => {
                                            Some(AmmoIndex::Bullet)
                                        }
                                        ItemClass::Launcher => Some(AmmoIndex::Throw),
                                        _ => None,
                                    };

                                    if let Some(ammo_index) = ammo_index {
                                        if equipment
                                            .get_ammo_slot_mut(ammo_index)
                                            .try_take_quantity(hit_count as u32)
                                            .is_none()
                                        {
                                            cancel_attack = true;
                                        } else if let Some(game_client) = game_client {
                                            match equipment.get_ammo_item(ammo_index) {
                                                Some(ammo_item) => {
                                                    if (ammo_item.quantity & 0x0F) == 0 {
                                                        game_client
                                                            .server_message_tx
                                                            .send(ServerMessage::UpdateInventory(
                                                                vec![(
                                                                    ItemSlot::Ammo(ammo_index),
                                                                    Some(Item::Stackable(
                                                                        ammo_item.clone(),
                                                                    )),
                                                                )],
                                                                None,
                                                            ))
                                                            .ok();
                                                    }
                                                }
                                                None => {
                                                    server_messages.send_entity_message(
                                                        client_entity,
                                                        ServerMessage::UpdateAmmo(
                                                            client_entity.id,
                                                            ammo_index,
                                                            None,
                                                        ),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            if cancel_attack {
                                // Not enough ammo, cancel attack
                                command_stop(
                                    &mut commands,
                                    &mut command,
                                    entity,
                                    client_entity,
                                    position,
                                    Some(&mut server_messages),
                                );
                                *next_command = NextCommand::default();
                            } else {
                                // In range, set current command to attack
                                *command = Command::with_attack(target_entity, attack_duration);

                                // Remove our destination component, as we have reached it!
                                entity_commands.remove::<Destination>();

                                // Update target
                                entity_commands.insert(Target::new(target_entity));

                                // Send damage event to damage system
                                damage_events.send(DamageEvent::with_attack(
                                    entity,
                                    target_entity,
                                    game_data.ability_value_calculator.calculate_damage(
                                        ability_values,
                                        target_ability_values,
                                        hit_count as i32,
                                    ),
                                ));
                            }
                        } else {
                            // Not in range, set current command to move
                            *command = Command::with_move(
                                target_position.position,
                                Some(target_entity),
                                Some(MoveMode::Run),
                            );

                            // Set destination to move towards
                            entity_commands.insert(Destination::new(target_position.position));

                            // Update target
                            entity_commands.insert(Target::new(target_entity));
                        }
                    } else {
                        command_stop(
                            &mut commands,
                            &mut command,
                            entity,
                            client_entity,
                            position,
                            Some(&mut server_messages),
                        );
                        *next_command = NextCommand::default();
                    }
                }
                &mut CommandData::CastSkill(CommandCastSkill {
                    skill_id,
                    skill_target,
                    ref use_item,
                    cast_motion_id,
                    action_motion_id,
                }) => {
                    if let Some(skill_data) = game_data.skills.get_skill(skill_id) {
                        let mut entity_commands = commands.entity(entity);
                        let (target_position, target_entity) = match skill_target {
                            Some(CommandCastSkillTarget::Entity(target_entity)) => {
                                if let Some((_, target_position, _)) = is_valid_skill_target(
                                    position,
                                    target_entity,
                                    &attack_target_query,
                                ) {
                                    (Some(target_position.position), Some(target_entity))
                                } else {
                                    (None, None)
                                }
                            }
                            Some(CommandCastSkillTarget::Position(target_position)) => (
                                Some(Point3::new(target_position.x, target_position.y, 0.0)),
                                None,
                            ),
                            None => (None, None),
                        };

                        let cast_range = if skill_data.cast_range > 0 {
                            skill_data.cast_range as f32
                        } else {
                            ability_values.get_attack_range() as f32
                        };

                        let in_distance = target_position
                            .map(|target_position| {
                                (target_position.xy() - position.position.xy()).magnitude()
                                    < cast_range as f32
                            })
                            .unwrap_or(true);
                        if in_distance {
                            let casting_duration = cast_motion_id
                                .or(skill_data.casting_motion_id)
                                .and_then(|motion_id| {
                                    if let Some(npc) = npc {
                                        game_data.npcs.get_npc_motion(npc.id, motion_id)
                                    } else {
                                        game_data.motions.find_first_character_motion(motion_id)
                                    }
                                })
                                .map(|motion_data| motion_data.duration)
                                .unwrap_or_else(|| Duration::from_secs(0))
                                .mul_f32(skill_data.casting_motion_speed);

                            let action_duration = action_motion_id
                                .or(skill_data.action_motion_id)
                                .and_then(|motion_id| {
                                    if let Some(npc) = npc {
                                        game_data.npcs.get_npc_motion(npc.id, motion_id)
                                    } else {
                                        game_data.motions.find_first_character_motion(motion_id)
                                    }
                                })
                                .map(|motion_data| motion_data.duration)
                                .unwrap_or_else(|| Duration::from_secs(0))
                                .mul_f32(skill_data.action_motion_speed);

                            // For skills which target an entity, we must send a message indicating start of skill
                            if target_entity.is_some() {
                                server_messages.send_entity_message(
                                    client_entity,
                                    ServerMessage::StartCastingSkill(client_entity.id),
                                );
                            }

                            // Send skill event for effect to be applied after casting motion
                            skill_events.send(SkillEvent::new(
                                entity,
                                server_time.now + casting_duration,
                                skill_id,
                                match skill_target {
                                    None => SkillEventTarget::Entity(entity),
                                    Some(CommandCastSkillTarget::Entity(target_entity)) => {
                                        SkillEventTarget::Entity(target_entity)
                                    }
                                    Some(CommandCastSkillTarget::Position(target_position)) => {
                                        SkillEventTarget::Position(target_position)
                                    }
                                },
                                use_item.clone(),
                            ));

                            // Update next command
                            match skill_data.action_mode {
                                SkillActionMode::Stop => *next_command = NextCommand::default(),
                                SkillActionMode::Attack => {
                                    *next_command =
                                        target_entity.map_or_else(NextCommand::default, |target| {
                                            NextCommand::with_command_skip_server_message(
                                                CommandData::Attack(CommandAttack { target }),
                                            )
                                        })
                                }
                                SkillActionMode::Restore => match command.command {
                                    CommandData::Stop(_)
                                    | CommandData::Move(_)
                                    | CommandData::Attack(_) => {
                                        *next_command =
                                            NextCommand::with_command_skip_server_message(
                                                command.command.clone(),
                                            )
                                    }
                                    CommandData::Die(_)
                                    | CommandData::Emote(_)
                                    | CommandData::PickupItemDrop(_)
                                    | CommandData::PersonalStore
                                    | CommandData::Sit(_)
                                    | CommandData::CastSkill(_) => {
                                        *next_command = NextCommand::default()
                                    }
                                },
                            }

                            // Set current command to cast skill
                            *command = Command::with_cast_skill(
                                skill_id,
                                skill_target,
                                casting_duration,
                                action_duration,
                            );

                            // Remove our destination component, as we have reached it!
                            entity_commands.remove::<Destination>();
                        } else {
                            // TODO: By changing command to move here we affect SkillActionMode::Restore

                            // Not in range, set current command to move
                            let target_position = target_position.unwrap();
                            *command = Command::with_move(
                                target_position,
                                target_entity,
                                Some(MoveMode::Run),
                            );

                            // Set destination to move towards
                            entity_commands.insert(Destination::new(target_position));
                        }

                        // Update target
                        if let Some(target_entity) = target_entity {
                            entity_commands.insert(Target::new(target_entity));
                        } else {
                            entity_commands.remove::<Target>();
                        }
                    }
                }
                CommandData::PersonalStore => {
                    let personal_store = personal_store.unwrap();
                    server_messages.send_entity_message(
                        client_entity,
                        ServerMessage::OpenPersonalStore(server::OpenPersonalStore {
                            entity_id: client_entity.id,
                            skin: personal_store.skin,
                            title: personal_store.title.clone(),
                        }),
                    );

                    *command = Command::with_personal_store();
                    *next_command = NextCommand::default();
                }
                CommandData::Sit(CommandSit::Sitting) => {
                    let duration = motion_data
                        .get_sit_sitting()
                        .map(|motion_data| motion_data.duration)
                        .unwrap_or_else(|| Duration::from_secs(0));

                    *command = Command::with_sitting(duration);
                    *next_command = NextCommand::default();

                    server_messages.send_entity_message(
                        client_entity,
                        ServerMessage::SitToggle(client_entity.id),
                    );
                }
                CommandData::Sit(CommandSit::Standing) => {
                    // The transition from Sit to Standing happens above
                    *next_command = NextCommand::default();
                }
                CommandData::Sit(CommandSit::Sit) => {
                    // The transition from Sitting to Sit happens above
                    *next_command = NextCommand::default();
                }
                CommandData::Emote(CommandEmote { motion_id, is_stop }) => {
                    let motion_data = if let Some(npc) = npc {
                        game_data.npcs.get_npc_motion(npc.id, *motion_id)
                    } else {
                        game_data.motions.find_first_character_motion(*motion_id)
                    };

                    // We wait to send emote message until now as client applies it immediately
                    server_messages.send_entity_message(
                        client_entity,
                        ServerMessage::UseEmote(server::UseEmote {
                            entity_id: client_entity.id,
                            motion_id: *motion_id,
                            is_stop: *is_stop,
                        }),
                    );

                    let duration = motion_data
                        .map(|motion_data| motion_data.duration)
                        .unwrap_or_else(|| Duration::from_secs(0));

                    *command = Command::with_emote(*motion_id, *is_stop, duration);
                    *next_command = NextCommand::default();
                }
                CommandData::Die(_) => {}
            }
        },
    );
}
