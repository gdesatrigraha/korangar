#[cfg(feature = "debug")]
use std::collections::HashSet;
use std::sync::Arc;

use cgmath::{Array, Matrix4, Point3, SquareMatrix, Vector2, Vector3};
use derive_new::new;
use korangar_audio::AudioEngine;
#[cfg(feature = "debug")]
use korangar_interface::windows::PrototypeWindow;
use korangar_util::collision::{Frustum, KDTree, Sphere, AABB};
use korangar_util::container::{SimpleKey, SimpleSlab};
use korangar_util::create_simple_key;
#[cfg(feature = "debug")]
use option_ext::OptionExt;
#[cfg(feature = "debug")]
use ragnarok_formats::map::EffectSource;
#[cfg(feature = "debug")]
use ragnarok_formats::map::MapData;
use ragnarok_formats::map::{LightSettings, LightSource, SoundSource, Tile, TileFlags, WaterSettings};
#[cfg(feature = "debug")]
use ragnarok_formats::transform::Transform;
use ragnarok_packets::ClientTick;

use super::{Entity, Object, PointLightId, PointLightManager, ResourceSet, ResourceSetBuffer};
#[cfg(feature = "debug")]
use super::{LightSourceExt, Model, PointLightSet};
#[cfg(feature = "debug")]
use crate::graphics::ModelBatch;
use crate::graphics::{Camera, EntityInstruction, IndicatorInstruction, ModelInstruction, Texture};
#[cfg(feature = "debug")]
use crate::graphics::{DebugAabbInstruction, DebugCircleInstruction, RenderSettings};
#[cfg(feature = "debug")]
use crate::interface::application::InterfaceSettings;
#[cfg(feature = "debug")]
use crate::interface::layout::{ScreenPosition, ScreenSize};
#[cfg(feature = "debug")]
use crate::renderer::MarkerRenderer;
use crate::{Buffer, Color, GameFileLoader, ModelVertex, TileVertex, WaterVertex, MAP_TILE_SIZE};

create_simple_key!(ObjectKey, "Key to an object inside the map");
create_simple_key!(LightSourceKey, "Key to an light source inside the map");

fn average_tile_height(tile: &Tile) -> f32 {
    (tile.upper_left_height + tile.upper_right_height + tile.lower_left_height + tile.lower_right_height) / 4.0
}

// MOVE
fn get_value(day_timer: f32, offset: f32, p: f32) -> f32 {
    let sin = (day_timer + offset).sin();
    sin.abs().powf(2.0 - p) / sin
}

fn get_channels(day_timer: f32, offset: f32, ps: [f32; 3]) -> Vector3<f32> {
    let red = get_value(day_timer, offset, ps[0]);
    let green = get_value(day_timer, offset, ps[1]);
    let blue = get_value(day_timer, offset, ps[2]);
    Vector3::new(red, green, blue)
}

fn color_from_channel(base_color: Color, channels: Vector3<f32>) -> Color {
    Color::rgb_u8(
        (base_color.red * channels.x) as u8,
        (base_color.green * channels.y) as u8,
        (base_color.blue * channels.z) as u8,
    )
}

fn get_directional_light_color_intensity(directional_color: Color, intensity: f32, day_timer: f32) -> (Color, f32) {
    let sun_offset = 0.0;
    let moon_offset = std::f32::consts::PI;

    let directional_channels = get_channels(day_timer, sun_offset, [0.8, 0.0, 0.25]) * 255.0;

    if directional_channels.x.is_sign_positive() {
        let directional_color = color_from_channel(directional_color, directional_channels);
        return (directional_color, f32::min(intensity * 1.5, 1.0));
    }

    let directional_channels = get_channels(day_timer, moon_offset, [0.3; 3]) * 255.0;
    let directional_color = color_from_channel(Color::rgb_u8(150, 150, 255), directional_channels);

    (directional_color, f32::min(intensity * 1.5, 1.0))
}

pub fn get_light_direction(day_timer: f32) -> Vector3<f32> {
    let sun_offset = -std::f32::consts::FRAC_PI_2;
    let c = (day_timer + sun_offset).cos();
    let s = (day_timer + sun_offset).sin();

    match c.is_sign_positive() {
        true => Vector3::new(s, c, -0.5),
        false => Vector3::new(s, -c, -0.5),
    }
}

#[cfg(feature = "debug")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MarkerIdentifier {
    Object(u32),
    LightSource(u32),
    SoundSource(u32),
    EffectSource(u32),
    Particle(u16, u16),
    Entity(u32),
    Shadow(u32),
}

#[cfg(feature = "debug")]
impl MarkerIdentifier {
    pub const SIZE: f32 = 1.5;
}

#[derive(new)]
pub struct Map {
    width: usize,
    height: usize,
    water_settings: Option<WaterSettings>,
    light_settings: LightSettings,
    tiles: Vec<Tile>,
    ground_vertex_offset: usize,
    ground_vertex_count: usize,
    vertex_buffer: Arc<Buffer<ModelVertex>>,
    water_vertex_buffer: Option<Buffer<WaterVertex>>,
    texture: Arc<Texture>,
    objects: SimpleSlab<ObjectKey, Object>,
    light_sources: SimpleSlab<LightSourceKey, LightSource>,
    sound_sources: Vec<SoundSource>,
    #[cfg(feature = "debug")]
    effect_sources: Vec<EffectSource>,
    tile_picker_vertex_buffer: Buffer<TileVertex>,
    #[cfg(feature = "debug")]
    tile_vertex_buffer: Arc<Buffer<ModelVertex>>,
    object_kdtree: KDTree<ObjectKey, AABB>,
    light_source_kdtree: KDTree<LightSourceKey, Sphere>,
    background_music_track_name: Option<String>,
    #[cfg(feature = "debug")]
    map_data: MapData,
}

impl Map {
    pub fn x_in_bounds(&self, x: usize) -> bool {
        x <= self.width
    }

    pub fn y_in_bounds(&self, y: usize) -> bool {
        y <= self.height
    }

    pub fn get_world_position(&self, position: Vector2<usize>) -> Point3<f32> {
        let height = average_tile_height(self.get_tile(position));
        Point3::new(position.x as f32 * 5.0 + 2.5, height, position.y as f32 * 5.0 + 2.5)
    }

    // TODO: Make this private once path finding is properly implemented
    pub fn get_tile(&self, position: Vector2<usize>) -> &Tile {
        &self.tiles[position.x + position.y * self.width]
    }

    pub fn background_music_track_name(&self) -> Option<&str> {
        self.background_music_track_name.as_deref()
    }

    pub fn get_texture(&self) -> &Arc<Texture> {
        &self.texture
    }

    pub fn get_model_vertex_buffer(&self) -> &Arc<Buffer<ModelVertex>> {
        &self.vertex_buffer
    }

    pub fn get_tile_picker_vertex_buffer(&self) -> &Buffer<TileVertex> {
        &self.tile_picker_vertex_buffer
    }

    pub fn set_ambient_sound_sources(&self, audio_engine: &AudioEngine<GameFileLoader>) {
        // We increase the range of the ambient sound,
        // so that it can ease better into the world.
        const AMBIENT_SOUND_MULTIPLIER: f32 = 1.5;

        // This is the only correct place to clear the ambient sound.
        audio_engine.clear_ambient_sound();

        for sound in self.sound_sources.iter() {
            let sound_effect_key = audio_engine.load(&sound.sound_file);

            audio_engine.add_ambient_sound(
                sound_effect_key,
                sound.position,
                sound.range * AMBIENT_SOUND_MULTIPLIER,
                sound.volume,
                sound.cycle,
            );
        }

        audio_engine.prepare_ambient_sound_world();
    }

    // We want to make sure that the object set also captures the lifetime of the
    // map, so we never have a stale object set.
    #[cfg_attr(feature = "debug", korangar_debug::profile)]
    pub fn cull_objects_with_frustum<'a>(
        &'a self,
        camera: &dyn Camera,
        object_set: &'a mut ResourceSetBuffer<ObjectKey>,
        #[cfg(feature = "debug")] enabled: bool,
    ) -> ResourceSet<'a, ObjectKey> {
        #[cfg(feature = "debug")]
        if !enabled {
            return object_set.create_set(|visible_objects| {
                self.objects.iter().for_each(|(object_key, _)| visible_objects.push(object_key));
            });
        }

        let (view_matrix, projection_matrix) = camera.view_projection_matrices();
        let frustum = Frustum::new(projection_matrix * view_matrix);

        object_set.create_set(|visible_objects| {
            self.object_kdtree.query(&frustum, visible_objects);
        })
    }

    // We want to make sure that the object set also caputres the lifetime of the
    // map, so we never have a stale object set.
    #[cfg_attr(feature = "debug", korangar_debug::profile)]
    pub fn cull_objects_in_sphere<'a>(
        &'a self,
        sphere: Sphere,
        object_set: &'a mut ResourceSetBuffer<ObjectKey>,
        #[cfg(feature = "debug")] enabled: bool,
    ) -> ResourceSet<'a, ObjectKey> {
        #[cfg(feature = "debug")]
        if !enabled {
            return object_set.create_set(|visible_objects| {
                self.objects.iter().for_each(|(object_key, _)| visible_objects.push(object_key));
            });
        }

        object_set.create_set(|visible_objects| {
            self.object_kdtree.query(&sphere, visible_objects);
        })
    }

    #[cfg_attr(feature = "debug", korangar_debug::profile)]
    pub fn render_objects(&self, instructions: &mut Vec<ModelInstruction>, object_set: &ResourceSet<ObjectKey>, client_tick: ClientTick) {
        for object_key in object_set.iterate_visible().copied() {
            if let Some(object) = self.objects.get(object_key) {
                object.render_geometry(instructions, client_tick);
            }
        }
    }

    #[cfg_attr(feature = "debug", korangar_debug::profile)]
    pub fn render_ground(&self, instructions: &mut Vec<ModelInstruction>) {
        instructions.push(ModelInstruction {
            model_matrix: Matrix4::identity(),
            vertex_offset: self.ground_vertex_offset,
            vertex_count: self.ground_vertex_count,
        });
    }

    #[cfg_attr(feature = "debug", korangar_debug::profile)]
    pub fn render_water<'a, 'b>(&'a self, water_vertex_buffer: &'b mut Option<&'a Buffer<WaterVertex>>) {
        *water_vertex_buffer = self.water_vertex_buffer.as_ref();
    }

    #[cfg_attr(feature = "debug", korangar_debug::profile)]
    pub fn render_entities(&self, instructions: &mut Vec<EntityInstruction>, entities: &[Entity], camera: &dyn Camera, include_self: bool) {
        entities
            .iter()
            .skip(!include_self as usize)
            .for_each(|entity| entity.render(instructions, camera));
    }

    #[cfg(feature = "debug")]
    #[korangar_debug::profile]
    pub fn render_bounding(
        &self,
        instructions: &mut Vec<DebugAabbInstruction>,
        frustum_culling: bool,
        object_set: &ResourceSet<ObjectKey>,
    ) {
        let intersection_set: HashSet<ObjectKey> = object_set.iterate_visible().copied().collect();

        self.objects.iter().for_each(|(object_key, object)| {
            let bounding_box_matrix = object.get_bounding_box_matrix();
            let bounding_box = AABB::from_transformation_matrix(bounding_box_matrix);
            let intersects = intersection_set.contains(&object_key);

            let color = match !frustum_culling || intersects {
                true => Color::rgb_u8(255, 255, 0),
                false => Color::rgb_u8(255, 0, 255),
            };

            let offset = bounding_box.size().y / 2.0;
            let position = bounding_box.center() - Vector3::new(0.0, offset, 0.0);
            let transform = Transform::position(position);
            let world_matrix = Model::bounding_box_matrix(&bounding_box, &transform);

            instructions.push(DebugAabbInstruction {
                world: world_matrix,
                color,
            });
        });
    }

    #[cfg_attr(feature = "debug", korangar_debug::profile)]
    pub fn render_walk_indicator(&self, instruction: &mut Option<IndicatorInstruction>, color: Color, position: Vector2<usize>) {
        const HALF_TILE_SIZE: f32 = MAP_TILE_SIZE / 2.0;
        const OFFSET: f32 = 1.0;

        // Since the picker buffer is always one frame behind the current scene, a map
        // transition can cause the picked tile to be out of bounds. To avoid a
        // panic we ensure the coordinates are in bounds.
        if position.x >= self.width || position.y >= self.height {
            return;
        }

        let tile = self.get_tile(position);

        if tile.flags.contains(TileFlags::WALKABLE) {
            let base_x = position.x as f32 * HALF_TILE_SIZE;
            let base_y = position.y as f32 * HALF_TILE_SIZE;

            let upper_left = Point3::new(base_x, tile.upper_left_height + OFFSET, base_y);
            let upper_right = Point3::new(base_x + HALF_TILE_SIZE, tile.upper_right_height + OFFSET, base_y);
            let lower_left = Point3::new(base_x, tile.lower_left_height + OFFSET, base_y + HALF_TILE_SIZE);
            let lower_right = Point3::new(
                base_x + HALF_TILE_SIZE,
                tile.lower_right_height + OFFSET,
                base_y + HALF_TILE_SIZE,
            );

            *instruction = Some(IndicatorInstruction {
                upper_left,
                upper_right,
                lower_left,
                lower_right,
                color,
            });
        }
    }

    #[cfg_attr(feature = "debug", korangar_debug::profile)]
    pub fn get_ambient_light_color(&self, day_timer: f32) -> Color {
        let sun_offset = 0.0;
        let ambient_channels = (get_channels(day_timer, sun_offset, [0.3, 0.2, 0.2]) * 0.55 + Vector3::from_value(0.65)) * 255.0;
        color_from_channel(self.light_settings.ambient_color.to_owned().unwrap().into(), ambient_channels)
    }

    #[cfg_attr(feature = "debug", korangar_debug::profile)]
    pub fn get_directional_light(&self, day_timer: f32) -> (Vector3<f32>, Color) {
        let light_direction = get_light_direction(day_timer);
        let (directional_color, intensity) = get_directional_light_color_intensity(
            self.light_settings.diffuse_color.to_owned().unwrap().into(),
            self.light_settings.light_intensity.unwrap(),
            day_timer,
        );
        let color = Color::rgb(
            directional_color.red * intensity,
            directional_color.green * intensity,
            directional_color.blue * intensity,
        );
        (light_direction, color)
    }

    #[cfg_attr(feature = "debug", korangar_debug::profile)]
    pub fn register_point_lights(
        &self,
        point_light_manager: &mut PointLightManager,
        light_source_set_buffer: &mut ResourceSetBuffer<LightSourceKey>,
        camera: &dyn Camera,
    ) {
        let (view_matrix, projection_matrix) = camera.view_projection_matrices();
        let frustum = Frustum::new(projection_matrix * view_matrix);

        let set = light_source_set_buffer.create_set(|buffer| {
            self.light_source_kdtree.query(&frustum, buffer);
        });

        for light_source_key in set.iterate_visible().copied() {
            let light_source = self.light_sources.get(light_source_key).unwrap();

            point_light_manager.register(
                PointLightId::new(light_source_key.key()),
                light_source.position,
                light_source.color.into(),
                light_source.range,
            );
        }
    }

    #[cfg_attr(feature = "debug", korangar_debug::profile)]
    pub fn get_water_light(&self) -> f32 {
        self.water_settings
            .as_ref()
            .and_then(|settings| settings.water_level)
            .unwrap_or_default()
    }

    #[cfg(feature = "debug")]
    pub fn to_prototype_window(&self) -> &dyn PrototypeWindow<InterfaceSettings> {
        &self.map_data
    }

    #[cfg(feature = "debug")]
    #[korangar_debug::profile]
    pub fn render_overlay_tiles(
        &self,
        model_instructions: &mut Vec<ModelInstruction>,
        model_batches: &mut Vec<ModelBatch>,
        tile_texture: &Arc<Texture>,
    ) {
        let vertex_count = self.tile_vertex_buffer.count() as usize;
        let offset = model_instructions.len();

        model_instructions.push(ModelInstruction {
            model_matrix: Matrix4::identity(),
            vertex_offset: 0,
            vertex_count,
        });

        model_batches.push(ModelBatch {
            offset,
            count: 1,
            texture: tile_texture.clone(),
            vertex_buffer: self.tile_vertex_buffer.clone(),
        });
    }

    #[cfg(feature = "debug")]
    #[korangar_debug::profile]
    pub fn render_entity_pathing(
        &self,
        model_instructions: &mut Vec<ModelInstruction>,
        model_batches: &mut Vec<ModelBatch>,
        entities: &[Entity],
        path_texture: &Arc<Texture>,
    ) {
        entities.iter().for_each(|entity| {
            if let Some(vertex_buffer) = entity.get_pathing_vertex_buffer() {
                let vertex_count = self.tile_vertex_buffer.count() as usize;
                let offset = model_instructions.len();

                model_instructions.push(ModelInstruction {
                    model_matrix: Matrix4::identity(),
                    vertex_offset: 0,
                    vertex_count,
                });

                model_batches.push(ModelBatch {
                    offset,
                    count: 1,
                    texture: path_texture.clone(),
                    vertex_buffer: vertex_buffer.clone(),
                });
            }
        });
    }

    #[cfg(feature = "debug")]
    pub fn resolve_marker<'a>(
        &'a self,
        entities: &'a [Entity],
        marker_identifier: MarkerIdentifier,
    ) -> &dyn PrototypeWindow<InterfaceSettings> {
        match marker_identifier {
            MarkerIdentifier::Object(key) => self.objects.get(ObjectKey::new(key)).unwrap(),
            MarkerIdentifier::LightSource(key) => self.light_sources.get(LightSourceKey::new(key)).unwrap(),
            MarkerIdentifier::SoundSource(index) => &self.sound_sources[index as usize],
            MarkerIdentifier::EffectSource(index) => &self.effect_sources[index as usize],
            MarkerIdentifier::Particle(..) => todo!(),
            MarkerIdentifier::Entity(index) => &entities[index as usize],
            MarkerIdentifier::Shadow(..) => todo!(),
        }
    }

    #[cfg(feature = "debug")]
    #[korangar_debug::profile]
    pub fn render_markers(
        &self,
        renderer: &mut impl MarkerRenderer,
        camera: &dyn Camera,
        render_settings: &RenderSettings,
        entities: &[Entity],
        point_light_set: &PointLightSet,
        hovered_marker_identifier: Option<MarkerIdentifier>,
    ) {
        use super::SoundSourceExt;
        use crate::EffectSourceExt;

        if render_settings.show_object_markers {
            self.objects.iter().for_each(|(object_key, object)| {
                let marker_identifier = MarkerIdentifier::Object(object_key.key());

                object.render_marker(
                    renderer,
                    camera,
                    marker_identifier,
                    hovered_marker_identifier.contains(&marker_identifier),
                )
            });
        }

        if render_settings.show_light_markers {
            self.light_sources.iter().for_each(|(key, light_source)| {
                let marker_identifier = MarkerIdentifier::LightSource(key.key());

                light_source.render_marker(
                    renderer,
                    camera,
                    marker_identifier,
                    hovered_marker_identifier.contains(&marker_identifier),
                )
            });
        }

        if render_settings.show_sound_markers {
            self.sound_sources.iter().enumerate().for_each(|(index, sound_source)| {
                let marker_identifier = MarkerIdentifier::SoundSource(index as u32);

                sound_source.render_marker(
                    renderer,
                    camera,
                    marker_identifier,
                    hovered_marker_identifier.contains(&marker_identifier),
                )
            });
        }

        if render_settings.show_effect_markers {
            self.effect_sources.iter().enumerate().for_each(|(index, effect_source)| {
                let marker_identifier = MarkerIdentifier::EffectSource(index as u32);

                effect_source.render_marker(
                    renderer,
                    camera,
                    marker_identifier,
                    hovered_marker_identifier.contains(&marker_identifier),
                )
            });
        }

        if render_settings.show_entity_markers {
            entities.iter().enumerate().for_each(|(index, entity)| {
                let marker_identifier = MarkerIdentifier::Entity(index as u32);

                entity.render_marker(
                    renderer,
                    camera,
                    marker_identifier,
                    hovered_marker_identifier.contains(&marker_identifier),
                )
            });
        }

        if render_settings.show_shadow_markers {
            point_light_set
                .with_shadow_iterator()
                .enumerate()
                .for_each(|(index, light_source)| {
                    let marker_identifier = MarkerIdentifier::Shadow(index as u32);

                    renderer.render_marker(
                        camera,
                        marker_identifier,
                        light_source.position,
                        hovered_marker_identifier.contains(&marker_identifier),
                    );
                });
        }
    }

    #[cfg(feature = "debug")]
    #[korangar_debug::profile]
    pub fn render_marker_overlay(
        &self,
        aabb_instructions: &mut Vec<DebugAabbInstruction>,
        circle_instructions: &mut Vec<DebugCircleInstruction>,
        camera: &dyn Camera,
        marker_identifier: MarkerIdentifier,
        point_light_set: &PointLightSet,
        animation_time: f32,
    ) {
        let offset = (f32::sin(animation_time * 5.0) + 0.5).clamp(0.0, 1.0);
        let overlay_color = Color::rgb(1.0, offset, 1.0 - offset);

        match marker_identifier {
            MarkerIdentifier::Object(key) => self
                .objects
                .get(ObjectKey::new(key))
                .unwrap()
                .render_bounding_box(aabb_instructions, overlay_color),

            MarkerIdentifier::LightSource(key) => {
                let light_source = self.light_sources.get(LightSourceKey::new(key)).unwrap();

                if let Some((screen_position, screen_size)) =
                    Self::calculate_circle_screen_position_size(camera, light_source.position, light_source.range)
                {
                    circle_instructions.push(DebugCircleInstruction {
                        position: light_source.position,
                        color: overlay_color,
                        screen_position,
                        screen_size,
                    });
                };
            }
            MarkerIdentifier::SoundSource(index) => {
                let sound_source = &self.sound_sources[index as usize];

                if let Some((screen_position, screen_size)) =
                    Self::calculate_circle_screen_position_size(camera, sound_source.position, sound_source.range)
                {
                    circle_instructions.push(DebugCircleInstruction {
                        position: sound_source.position,
                        color: overlay_color,
                        screen_position,
                        screen_size,
                    });
                };
            }
            MarkerIdentifier::EffectSource(_index) => {}
            MarkerIdentifier::Particle(_index, _particle_index) => {}
            MarkerIdentifier::Entity(_index) => {}
            MarkerIdentifier::Shadow(index) => {
                let point_light = point_light_set.with_shadow_iterator().nth(index as usize).unwrap();

                if let Some((screen_position, screen_size)) =
                    Self::calculate_circle_screen_position_size(camera, point_light.position, point_light.range)
                {
                    circle_instructions.push(DebugCircleInstruction {
                        position: point_light.position,
                        color: overlay_color,
                        screen_position,
                        screen_size,
                    });
                };
            }
        }
    }

    #[cfg(feature = "debug")]
    fn calculate_circle_screen_position_size(
        camera: &dyn Camera,
        position: Point3<f32>,
        extent: f32,
    ) -> Option<(ScreenPosition, ScreenSize)> {
        let corner_offset = (extent.powf(2.0) * 2.0).sqrt();
        let (top_left_position, bottom_right_position) = camera.billboard_coordinates(position, corner_offset);

        if top_left_position.w < 0.1 && bottom_right_position.w < 0.1 && camera.distance_to(position) > extent {
            return None;
        }

        let (screen_position, screen_size) = camera.screen_position_size(top_left_position, bottom_right_position);
        Some((screen_position, screen_size))
    }
}
