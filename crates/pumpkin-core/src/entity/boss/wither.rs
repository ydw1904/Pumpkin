use crossbeam::atomic::AtomicCell;
use pumpkin_util::random::gameplay::game_random;
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};
use uuid::Uuid;

use pumpkin_data::{
    Block, BlockState,
    attributes::Attributes,
    damage::DamageType,
    entity::EntityType,
    item::Item,
    item_stack::ItemStack,
    tag::{self, Taggable},
    tracked_data,
    world::WorldEvent,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::{
    Difficulty,
    math::{position::BlockPos, vector3::Vector3},
    text::TextComponent,
};
use pumpkin_world::world::BlockFlags;

use crate::{
    entity::{
        Entity, EntityBase,
        ai::behavior::hunting::{self, Hunt},
        ai::control::flying_move_control::FlyingMoveControl,
        ai::goal::{
            Controls, Goal, GoalReach, look_around::RandomLookAroundGoal,
            look_at_entity::LookAtEntityGoal, ranged_attack::RangedAttackGoal,
            revenge::RevengeGoal, water_avoiding_random_flying::WaterAvoidingRandomFlyingGoal,
        },
        ai::pathfinder::Navigator,
        ai::target_match::TargetMatch,
        ai::target_predicate::TargetPredicate,
        living::LivingEntity,
        mob::{Mob, MobEntity, RangedAttackMob},
        projectile::wither_skull::WitherSkullEntity,
    },
    world::{
        ExplosionInteraction, World,
        bossbar::{Bossbar, BossbarColor, BossbarDivisions, BossbarFlags},
    },
};

const INVULNERABLE_TICKS: i32 = 220;

const WITHER_HUNTS: &[Hunt] = &[Hunt::new(
    2,
    TargetMatch::AllExcept(&tag::EntityType::MINECRAFT_WITHER_FRIENDS),
    false,
)
.every(0)
.matching(|_hunter, target, _world| target.attackable())];

pub struct WitherEntity {
    pub mob_entity: MobEntity,
    pub invulnerable_ticks: AtomicI32,
    pub destroy_blocks_tick: AtomicI32,
    pub next_head_update: [AtomicI32; 2],
    pub idle_head_updates: [AtomicI32; 2],
    pub alternative_targets: [AtomicI32; 3],
    pub bossbar_uuid: Uuid,
    pub bossbar_players: Mutex<Vec<Uuid>>,
    bossbar_progress: AtomicCell<f32>,
    bossbar_title: Mutex<TextComponent>,
    pub dropped_loot: AtomicBool,
}

impl WitherEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `WitherBoss.defineSynchedData`.
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::wither::DATA_TARGET_A, VarInt(0));
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::wither::DATA_TARGET_B, VarInt(0));
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::wither::DATA_TARGET_C, VarInt(0));
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::wither::DATA_ID_INV, VarInt(0));
        let mut mob_entity = MobEntity::new(entity);
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(FlyingMoveControl::new(10, false));
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *navigator = Navigator::flying();
        navigator.set_can_open_doors(false);
        navigator.set_can_float(true);
        let wither = Self {
            mob_entity,
            invulnerable_ticks: AtomicI32::new(0),
            destroy_blocks_tick: AtomicI32::new(0),
            next_head_update: [AtomicI32::new(0), AtomicI32::new(0)],
            idle_head_updates: [AtomicI32::new(0), AtomicI32::new(0)],
            alternative_targets: [AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0)],
            bossbar_uuid: Uuid::new_v4(),
            bossbar_players: Mutex::new(Vec::new()),
            bossbar_progress: AtomicCell::new(1.0),
            bossbar_title: Mutex::new(Self::default_bossbar_title()),
            dropped_loot: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(wither);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let ranged_weak: Weak<dyn RangedAttackMob> = {
            let ranged_arc: Arc<dyn RangedAttackMob> = mob_arc.clone();
            Arc::downgrade(&ranged_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let target_selector = &mob_arc.mob_entity.target_selector;

            goal_selector.add_goal(
                0,
                Box::new(WitherDoNothingGoal {
                    wither: Arc::downgrade(&mob_arc),
                }),
            );
            goal_selector.add_goal(
                2,
                Box::new(RangedAttackGoal::new(ranged_weak, 1.0, 40, 20.0)),
            );
            goal_selector.add_goal(5, Box::new(WaterAvoidingRandomFlyingGoal::new(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));

            target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
            hunting::apply(target_selector, &mob_arc.mob_entity, WITHER_HUNTS);
        };

        mob_arc
    }

    #[must_use]
    pub fn get_invulnerable_ticks(&self) -> i32 {
        self.invulnerable_ticks.load(Ordering::Relaxed)
    }

    pub fn set_invulnerable_ticks(&self, ticks: i32) {
        self.invulnerable_ticks.store(ticks, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(tracked_data::wither::DATA_ID_INV, VarInt(ticks));
    }

    #[must_use]
    pub fn get_alternative_target(&self, head: usize) -> i32 {
        if head < 3 {
            self.alternative_targets[head].load(Ordering::Relaxed)
        } else {
            0
        }
    }

    pub fn set_alternative_target(&self, head: usize, entity_id: i32) {
        if head < 3 {
            let old = self.alternative_targets[head].swap(entity_id, Ordering::Relaxed);
            if old != entity_id {
                let tracker_id = match head {
                    0 => tracked_data::wither::DATA_TARGET_A,
                    1 => tracked_data::wither::DATA_TARGET_B,
                    _ => tracked_data::wither::DATA_TARGET_C,
                };
                self.mob_entity
                    .living_entity
                    .entity
                    .set_synced_data(tracker_id, VarInt(entity_id));
            }
        }
    }

    #[must_use]
    pub fn is_powered(&self) -> bool {
        let living = &self.mob_entity.living_entity;
        living.health.load() <= living.get_max_health() / 2.0
    }

    pub fn make_invulnerable(&self) {
        self.set_invulnerable_ticks(INVULNERABLE_TICKS);
        self.bossbar_progress.store(0.0);
        self.mob_entity
            .living_entity
            .set_health(self.mob_entity.living_entity.get_max_health() / 3.0);
    }

    #[must_use]
    pub fn can_destroy(block: &Block, state: &BlockState) -> bool {
        !state.is_air() && !block.has_tag(&tag::Block::MINECRAFT_WITHER_IMMUNE)
    }

    fn scale(&self) -> f64 {
        self.mob_entity
            .living_entity
            .get_attribute_value(&Attributes::SCALE)
    }

    /// Vanilla `getHeadX`. Index 3 lands on the same spot as index 1.
    #[must_use]
    pub fn get_head_x(&self, head: usize) -> f64 {
        let entity = &self.mob_entity.living_entity.entity;
        if head == 0 {
            return entity.pos.load().x;
        }
        let angle = (entity.body_yaw.load() + 180.0 * (head as f32 - 1.0)).to_radians();
        entity.pos.load().x + f64::from(angle.cos()) * 1.3 * self.scale()
    }

    #[must_use]
    pub fn get_head_y(&self, head: usize) -> f64 {
        let height: f32 = if head == 0 { 3.0 } else { 2.2 };
        self.mob_entity.living_entity.entity.pos.load().y + f64::from(height) * self.scale()
    }

    #[must_use]
    pub fn get_head_z(&self, head: usize) -> f64 {
        let entity = &self.mob_entity.living_entity.entity;
        if head == 0 {
            return entity.pos.load().z;
        }
        let angle = (entity.body_yaw.load() + 180.0 * (head as f32 - 1.0)).to_radians();
        entity.pos.load().z + f64::from(angle.sin()) * 1.3 * self.scale()
    }

    /// Vanilla `performRangedAttack(int, LivingEntity)`: only the middle head can roll a
    /// dangerous skull.
    fn perform_ranged_attack_on(&self, head: usize, target: &dyn EntityBase) {
        let target = target.get_entity();
        let pos = target.pos.load();
        let dangerous = head == 0 && game_random().random::<f32>() < 0.001;
        self.perform_ranged_attack_at(
            head,
            pos.x,
            pos.y + target.get_eye_height() * 0.5,
            pos.z,
            dangerous,
        );
    }

    fn perform_ranged_attack_at(
        &self,
        head: usize,
        target_x: f64,
        target_y: f64,
        target_z: f64,
        dangerous: bool,
    ) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();

        if !entity.silent.load(Ordering::Relaxed) {
            world.sync_world_event(WorldEvent::SoundWitherBossShoot, entity.block_pos.load(), 0);
        }

        let hx = self.get_head_x(head);
        let hy = self.get_head_y(head);
        let hz = self.get_head_z(head);
        let head_pos = Vector3::new(hx, hy, hz);
        let direction = Vector3::new(target_x - hx, target_y - hy, target_z - hz).normalize();

        let skull_entity = Entity::from_uuid(
            Uuid::new_v4(),
            (*world).clone(),
            head_pos,
            &EntityType::WITHER_SKULL,
        );
        let skull = WitherSkullEntity::new_shot(skull_entity, entity, direction);
        if dangerous {
            skull.set_dangerous(true);
        }
        // `ThrownItemEntity::new` puts the skull at the shooter's eye; vanilla `setPos` moves it
        // to the head.
        skull.thrown.entity.pos.store(head_pos);
        world.spawn_entity(Arc::new(skull));
    }

    fn default_bossbar_title() -> TextComponent {
        TextComponent::translate_cross("entity.minecraft.wither", "entity.minecraft.wither", [])
    }

    fn bossbar_title_now(&self) -> TextComponent {
        self.mob_entity
            .living_entity
            .entity
            .custom_name
            .load()
            .as_ref()
            .clone()
            .unwrap_or_else(Self::default_bossbar_title)
    }

    fn make_bossbar(&self) -> Bossbar {
        let title = self.bossbar_title_now();
        Bossbar {
            uuid: self.bossbar_uuid,
            title,
            health: 1.0,
            color: BossbarColor::Purple,
            division: BossbarDivisions::NoDivision,
            flags: BossbarFlags::DARKEN_SKY,
        }
    }

    fn update_bossbar(&self, world: &Arc<crate::world::World>, progress: f32) {
        let players = world.players.load();
        // Vanilla `startSeenByPlayer` / `stopSeenByPlayer`: the bar follows entity tracking.
        let tracked = world
            .entity_tracker
            .get_tracked_entity(self.mob_entity.living_entity.entity.entity_id);
        let current: Vec<Uuid> = players
            .iter()
            .map(|p| p.gameprofile.id)
            .filter(|uid| tracked.as_ref().is_some_and(|t| t.is_seen_by(uid)))
            .collect();

        let mut bossbar_players = self
            .bossbar_players
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        for &uid in &current {
            if !bossbar_players.contains(&uid) {
                if let Some(p) = players.iter().find(|p| p.gameprofile.id == uid) {
                    let mut bar = self.make_bossbar();
                    bar.health = progress;
                    p.send_bossbar(&bar);
                }
                bossbar_players.push(uid);
            }
        }

        let to_remove: Vec<Uuid> = bossbar_players
            .iter()
            .filter(|uid| !current.contains(uid))
            .copied()
            .collect();

        for uid in &to_remove {
            if let Some(p) = players.iter().find(|p| &p.gameprofile.id == uid) {
                p.remove_bossbar(self.bossbar_uuid);
            }
            bossbar_players.retain(|u| u != uid);
        }

        // WitherBoss.setCustomName renames the boss bar, so a name tag shows up right away.
        let title = self.bossbar_title_now();
        let mut sent_title = self
            .bossbar_title
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let title_changed = *sent_title != title;
        if title_changed {
            sent_title.clone_from(&title);
        }

        for player in players.iter() {
            if bossbar_players.contains(&player.gameprofile.id) {
                if title_changed {
                    player.update_bossbar_title(&self.bossbar_uuid, title.clone());
                }
                player.update_bossbar_health(&self.bossbar_uuid, progress);
            }
        }
    }

    fn remove_all_bossbar(&self, world: &Arc<crate::world::World>) {
        let mut bossbar_players = self
            .bossbar_players
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let players = world.players.load();
        for player in players.iter() {
            if bossbar_players.contains(&player.gameprofile.id) {
                player.remove_bossbar(self.bossbar_uuid);
            }
        }
        bossbar_players.clear();
    }

    /// The start of vanilla `WitherBoss.aiStep`, before `super.aiStep()`.
    fn tick_wither_movement(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let mut movement = entity.velocity.load().multiply(1.0, 0.6, 1.0);
        let target_id = self.get_alternative_target(0);
        if target_id > 0
            && let Some(target) = entity.world.load().get_entity_by_id(target_id)
        {
            let target_pos = target.get_entity().pos.load();
            let pos = entity.pos.load();
            if pos.y < target_pos.y || (!self.is_powered() && pos.y < target_pos.y + 5.0) {
                movement.y = movement.y.max(0.0);
                movement.y += 0.3 - movement.y * f64::from(0.6f32);
            }
            let delta = Vector3::new(target_pos.x - pos.x, 0.0, target_pos.z - pos.z);
            if delta.horizontal_length_squared() > 9.0 {
                let direction = delta.normalize();
                movement.x += direction.x * 0.3 - movement.x * 0.6;
                movement.z += direction.z * 0.3 - movement.z * 0.6;
            }
        }
        entity.velocity.store(movement);
        if movement.horizontal_length_squared() > 0.05 {
            let yaw = (movement.z.atan2(movement.x).to_degrees() as f32) - 90.0;
            entity.set_rotation(yaw, entity.pitch.load());
        }
    }

    /// Vanilla `WitherBoss.checkDespawn`.
    fn check_wither_despawn(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();

        if world.level_info.load().difficulty == Difficulty::Peaceful {
            self.remove_all_bossbar(&world);
            entity.remove();
            return;
        }

        // Vanilla `checkDespawn` keeps the wither from ever idling out.
        self.mob_entity
            .living_entity
            .no_action_time
            .store(0, Ordering::Relaxed);
    }

    fn tick_wither(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let invul = self.get_invulnerable_ticks();
        let tick_count = entity.tick_count.load(Ordering::Relaxed);

        if invul > 0 {
            let new_count = invul - 1;
            let progress = (1.0 - (new_count as f32) / INVULNERABLE_TICKS as f32).clamp(0.0, 1.0);
            self.bossbar_progress.store(progress);

            if new_count <= 0 {
                let pos = entity.pos.load();
                let eye_y = pos.y + entity.get_eye_height();
                world.explode(
                    Some(self),
                    Vector3::new(pos.x, eye_y, pos.z),
                    7.0,
                    ExplosionInteraction::Mob,
                );

                if !entity.silent.load(Ordering::Relaxed) {
                    world.sync_global_world_event(
                        WorldEvent::SoundWitherBossSpawn,
                        entity.block_pos.load(),
                        0,
                    );
                }
            }

            self.set_invulnerable_ticks(new_count);
            if tick_count % 10 == 0 {
                self.mob_entity.living_entity.heal(10.0);
            }
        } else {
            self.tick_side_heads(&world, tick_count);

            let target_id = self
                .mob_entity
                .get_target()
                .map_or(0, |target| target.get_entity().entity_id);
            self.set_alternative_target(0, target_id);

            self.tick_destroy_blocks(&world);

            let living = &self.mob_entity.living_entity;
            if tick_count % 20 == 0 {
                living.heal(1.0);
            }
            self.bossbar_progress
                .store(living.health.load() / living.get_max_health());
        }
    }

    /// The side-head loop of vanilla `customServerAiStep`. Head `i` keeps target `i` but, like
    /// vanilla, shoots from head index `i + 1`.
    fn tick_side_heads(&self, world: &Arc<World>, tick_count: i32) {
        let entity = &self.mob_entity.living_entity.entity;
        let difficulty = world.level_info.load().difficulty;

        for i in 1..3 {
            if tick_count < self.next_head_update[i - 1].load(Ordering::Relaxed) {
                continue;
            }
            self.next_head_update[i - 1].store(
                tick_count + 10 + game_random().random_range(0..10),
                Ordering::Relaxed,
            );

            if (difficulty == Difficulty::Normal || difficulty == Difficulty::Hard)
                && self.idle_head_updates[i - 1].fetch_add(1, Ordering::Relaxed) > 15
            {
                let pos = entity.pos.load();
                let (xt, yt, zt) = {
                    let rng = game_random();
                    (
                        rng.random_range(pos.x - 10.0..pos.x + 10.0),
                        rng.random_range(pos.y - 5.0..pos.y + 5.0),
                        rng.random_range(pos.z - 10.0..pos.z + 10.0),
                    )
                };
                self.perform_ranged_attack_at(i + 1, xt, yt, zt, true);
                self.idle_head_updates[i - 1].store(0, Ordering::Relaxed);
            }

            let head_target_id = self.get_alternative_target(i);
            if head_target_id > 0 {
                let current = world
                    .get_entity_by_id(head_target_id)
                    .filter(|target| target.get_living_entity().is_some());
                if let Some(current) = current
                    && self.can_attack(current.as_ref())
                    && entity
                        .pos
                        .load()
                        .squared_distance_to_vec(&current.get_entity().pos.load())
                        <= 900.0
                    && self.has_line_of_sight(current.get_entity())
                {
                    self.perform_ranged_attack_on(i + 1, current.as_ref());
                    self.next_head_update[i - 1].store(
                        tick_count + 40 + game_random().random_range(0..20),
                        Ordering::Relaxed,
                    );
                    self.idle_head_updates[i - 1].store(0, Ordering::Relaxed);
                } else {
                    self.set_alternative_target(i, 0);
                }
            } else {
                // Vanilla `TARGETING_CONDITIONS`: combat test within 20, players included.
                let search_box = entity.bounding_box.load().expand(20.0, 8.0, 20.0);
                let predicate = TargetPredicate::create_attackable().set_base_max_distance(20.0);
                let mut candidates = world.get_entities_at_box(&search_box);
                candidates.extend(
                    world
                        .get_players_at_box(&search_box)
                        .into_iter()
                        .map(|player| player as Arc<dyn EntityBase>),
                );
                candidates.retain(|candidate| {
                    !candidate
                        .get_entity()
                        .entity_type
                        .has_tag(&tag::EntityType::MINECRAFT_WITHER_FRIENDS)
                        && candidate
                            .get_living_entity()
                            .is_some_and(LivingEntity::attackable)
                        && predicate.test(world, Some(self), candidate.as_ref())
                });

                if !candidates.is_empty() {
                    let idx = game_random().random_range(0..candidates.len());
                    self.set_alternative_target(i, candidates[idx].get_entity().entity_id);
                }
            }
        }
    }

    /// Vanilla `destroyBlocksTick` handling: a hurt wither breaks the blocks around its body a
    /// second later, dropping them like `destroyBlock(pos, true, this)`.
    fn tick_destroy_blocks(&self, world: &Arc<World>) {
        let destroy_tick = self.destroy_blocks_tick.load(Ordering::Relaxed);
        if destroy_tick <= 0 {
            return;
        }
        self.destroy_blocks_tick
            .store(destroy_tick - 1, Ordering::Relaxed);
        if destroy_tick - 1 != 0 || !world.level_info.load().game_rules.mob_griefing {
            return;
        }

        let entity = &self.mob_entity.living_entity.entity;
        let dimension = entity.entity_dimension.load();
        let width = (dimension.width / 2.0 + 1.0).floor() as i32;
        let height = dimension.height.floor() as i32;
        let block_pos = entity.block_pos.load();
        let mut destroyed = false;
        for pos in BlockPos::between_closed(
            BlockPos::new(block_pos.0.x - width, block_pos.0.y, block_pos.0.z - width),
            BlockPos::new(
                block_pos.0.x + width,
                block_pos.0.y + height,
                block_pos.0.z + width,
            ),
        ) {
            let (block, state) = world.get_block_and_state(&pos);
            if Self::can_destroy(block, state) {
                destroyed |= world
                    .break_block(&pos, None, BlockFlags::NOTIFY_ALL)
                    .is_some();
            }
        }

        if destroyed {
            world.sync_world_event(WorldEvent::SoundWitherBlockBreak, block_pos, 0);
        }
    }
}

impl RangedAttackMob for WitherEntity {
    fn perform_ranged_attack(&self, target: &Arc<dyn EntityBase>, _power: f32) {
        self.perform_ranged_attack_on(0, target.as_ref());
    }
}

/// Vanilla `WitherDoNothingGoal`: holds every control while the wither charges up.
struct WitherDoNothingGoal {
    wither: Weak<WitherEntity>,
}

impl Goal for WitherDoNothingGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        self.wither
            .upgrade()
            .is_some_and(|wither| wither.get_invulnerable_ticks() > 0)
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::JUMP | Controls::LOOK
    }

    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        Some(GoalReach::default())
    }
}

impl Mob for WitherEntity {
    fn on_removed(&self) {
        self.remove_all_bossbar(&self.get_entity().world.load());
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        entity.set_synced_data(
            tracked_data::wither::DATA_TARGET_A,
            VarInt(self.get_alternative_target(0)),
        );
        entity.set_synced_data(
            tracked_data::wither::DATA_TARGET_B,
            VarInt(self.get_alternative_target(1)),
        );
        entity.set_synced_data(
            tracked_data::wither::DATA_TARGET_C,
            VarInt(self.get_alternative_target(2)),
        );
        entity.set_synced_data(
            tracked_data::wither::DATA_ID_INV,
            VarInt(self.get_invulnerable_ticks()),
        );
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.check_wither_despawn();
    }

    fn requires_custom_persistence(&self) -> bool {
        true
    }

    fn before_mob_ai_step(&self) {
        self.tick_wither_movement();
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.tick_wither();
    }

    /// Vanilla `WitherBoss.hurtServer`, up to `super.hurtServer`.
    fn pre_damage_with_context(
        &self,
        damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let cause_type = cause.map(|cause| cause.get_entity().entity_type);
        if damage_type.has_tag(&tag::DamageType::MINECRAFT_WITHER_IMMUNE_TO)
            || cause_type == Some(&EntityType::WITHER)
        {
            return false;
        }

        if self.get_invulnerable_ticks() > 0
            && !damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_INVULNERABILITY)
        {
            return false;
        }

        // `AbstractArrow` covers tridents; the breeze's charge is not a `WindCharge`.
        if self.is_powered()
            && source.is_some_and(|source| {
                let source_type = source.get_entity().entity_type;
                source_type == &EntityType::ARROW
                    || source_type == &EntityType::SPECTRAL_ARROW
                    || source_type == &EntityType::TRIDENT
                    || source_type == &EntityType::WIND_CHARGE
            })
        {
            return false;
        }

        if cause_type.is_some_and(|ty| ty.has_tag(&tag::EntityType::MINECRAFT_WITHER_FRIENDS)) {
            return false;
        }

        if self.destroy_blocks_tick.load(Ordering::Relaxed) <= 0 {
            self.destroy_blocks_tick.store(20, Ordering::Relaxed);
        }
        for idle in &self.idle_head_updates {
            idle.fetch_add(3, Ordering::Relaxed);
        }
        true
    }

    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        let living = &self.mob_entity.living_entity;
        // Vanilla `dropCustomDeathLoot` runs from `LivingEntity.die` inside the lethal hit,
        // only with `mob_drops` on. Dropping here instead of on a later tick means a save
        // between the hit and that tick can't lose the star; `dead` is never persisted.
        // Two lethal hits from parallel entity ticks can both get here, so drop once.
        if living.dead.load(Ordering::Relaxed) && !self.dropped_loot.swap(true, Ordering::SeqCst) {
            let world = living.entity.world.load_full();
            if world.level_info.load().game_rules.mob_drops
                && let Some(nether_star) = living
                    .entity
                    .spawn_at_location(ItemStack::new(1, &Item::NETHER_STAR))
            {
                nether_star.set_extended_lifetime();
            }
        }
    }

    fn post_tick(&self) {
        // The bar stays through the death animation until the wither is removed.
        let entity = &self.mob_entity.living_entity.entity;
        self.update_bossbar(&entity.world.load(), self.bossbar_progress.load());
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("Invul", self.get_invulnerable_ticks());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(invul) = nbt.get_int("Invul") {
            self.set_invulnerable_ticks(invul);
        }
    }
}
