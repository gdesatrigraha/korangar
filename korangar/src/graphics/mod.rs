mod buffer;
mod cameras;
mod capabilities;
mod color;
mod engine;
#[cfg(feature = "debug")]
mod error;
mod frame_pacer;
mod graphic_settings;
mod instruction;
mod particles;
mod passes;
mod picker_target;
#[cfg(feature = "debug")]
mod render_settings;
mod sampler;
mod smoothed;
mod surface;
mod texture;
mod vertices;

use std::num::NonZeroU64;
use std::sync::{Arc, OnceLock};

use bytemuck::{Pod, Zeroable};
use cgmath::{Matrix4, SquareMatrix, Zero};
use wgpu::util::StagingBelt;
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource,
    BindingType, BlendComponent, BlendFactor, BlendOperation, BlendState, BufferBindingType, BufferUsages, CommandEncoder, Device,
    Extent3d, Queue, Sampler, SamplerBindingType, ShaderStages, StorageTextureAccess, TextureDescriptor, TextureDimension, TextureFormat,
    TextureSampleType, TextureUsages, TextureViewDimension, COPY_BYTES_PER_ROW_ALIGNMENT,
};

pub use self::buffer::Buffer;
pub use self::cameras::*;
pub use self::capabilities::*;
pub use self::color::*;
pub use self::engine::{GraphicsEngine, GraphicsEngineDescriptor};
#[cfg(feature = "debug")]
pub use self::error::error_handler;
pub use self::frame_pacer::*;
pub use self::graphic_settings::*;
pub use self::instruction::*;
pub use self::particles::*;
pub use self::picker_target::PickerTarget;
#[cfg(feature = "debug")]
pub use self::render_settings::*;
pub use self::smoothed::*;
pub use self::surface::*;
pub use self::texture::*;
pub use self::vertices::*;
use crate::graphics::passes::DispatchIndirectArgs;
use crate::graphics::sampler::create_new_sampler;
use crate::interface::layout::ScreenSize;
use crate::loaders::TextureLoader;
use crate::NUMBER_OF_POINT_LIGHTS_WITH_SHADOWS;

/// The size of a tile in pixel of the tile based light culling.
const LIGHT_TILE_SIZE: u32 = 16;

/// This texture format needs following requirements:
///  - Store alpha (forward shader)
///  - Usable as storage texture (post-processing shader)
///
/// Bot requirements are needed at the same time, since we want to be able to
/// re-use the forward color texture, if possible.
pub const RENDER_TO_TEXTURE_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

pub const INTERFACE_TEXTURE_FORMAT: TextureFormat = TextureFormat::Rgba8UnormSrgb;
pub const FXAA_COLOR_LUMA_TEXTURE_FORMAT: TextureFormat = TextureFormat::Rgba8UnormSrgb;

pub const MAX_BUFFER_SIZE: u64 = 128 * 1024 * 1024;

pub const WATER_ATTACHMENT_BLEND: BlendState = BlendState {
    color: BlendComponent {
        src_factor: BlendFactor::One,
        dst_factor: BlendFactor::One,
        operation: BlendOperation::ReverseSubtract,
    },
    alpha: BlendComponent {
        src_factor: BlendFactor::One,
        dst_factor: BlendFactor::One,
        operation: BlendOperation::Max,
    },
};

/// Trait to prepare all GPU data of contexts, computer and renderer.
pub(crate) trait Prepare {
    /// Prepares the GPU data.
    fn prepare(&mut self, _device: &Device, _instructions: &RenderInstruction);

    /// Stages the prepared data inside the staging belt.
    fn upload(&mut self, _device: &Device, _staging_belt: &mut StagingBelt, _command_encoder: &mut CommandEncoder);
}

#[derive(Copy, Clone, Default, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct GlobalUniforms {
    view_projection: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    inverse_view: [[f32; 4]; 4],
    inverse_projection: [[f32; 4]; 4],
    indicator_positions: [[f32; 4]; 4],
    indicator_color: [f32; 4],
    ambient_color: [f32; 4],
    screen_size: [u32; 2],
    pointer_position: [u32; 2],
    animation_timer: f32,
    day_timer: f32,
    water_level: f32,
    point_light_count: u32,
}

#[derive(Copy, Clone, Default, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct DirectionalLightUniforms {
    view_projection: [[f32; 4]; 4],
    color: [f32; 4],
    direction: [f32; 4],
}

#[derive(Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct PointLightData {
    position: [f32; 4],
    color: [f32; 4],
    range: f32,
    texture_index: i32,
    padding: [u32; 2],
}

#[cfg(feature = "debug")]
#[derive(Copy, Clone, Default, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct DebugUniforms {
    show_picker_buffer: u32,
    show_directional_shadow_map: u32,
    show_point_shadow_map: u32,
    show_light_culling_count_buffer: u32,
    show_font_atlas: u32,
}

#[derive(Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct TileLightIndices {
    indices: [u32; 256],
}

/// Holds all GPU resources that are shared by multiple passes.
pub(crate) struct GlobalContext {
    pub(crate) surface_texture_format: TextureFormat,
    pub(crate) msaa: Msaa,
    pub(crate) screen_space_anti_aliasing: ScreenSpaceAntiAliasing,
    pub(crate) solid_pixel_texture: Arc<Texture>,
    pub(crate) walk_indicator_texture: Arc<Texture>,
    pub(crate) forward_depth_texture: AttachmentTexture,
    pub(crate) picker_buffer_texture: AttachmentTexture,
    pub(crate) picker_depth_texture: AttachmentTexture,
    pub(crate) forward_color_texture: AttachmentTexture,
    pub(crate) resolved_color_texture: Option<AttachmentTexture>,
    pub(crate) interface_buffer_texture: AttachmentTexture,
    pub(crate) directional_shadow_map_texture: AttachmentTexture,
    pub(crate) point_shadow_map_textures: CubeArrayTexture,
    pub(crate) tile_light_count_texture: StorageTexture,
    pub(crate) global_uniforms_buffer: Buffer<GlobalUniforms>,
    pub(crate) directional_light_uniforms_buffer: Buffer<DirectionalLightUniforms>,
    pub(crate) point_light_data_buffer: Buffer<PointLightData>,
    #[cfg(feature = "debug")]
    pub(crate) debug_uniforms_buffer: Buffer<DebugUniforms>,
    pub(crate) picker_value_buffer: Buffer<u64>,
    pub(crate) tile_light_indices_buffer: Buffer<TileLightIndices>,
    pub(crate) anti_aliasing_resources: AntiAliasingResource,
    pub(crate) nearest_sampler: Sampler,
    pub(crate) linear_sampler: Sampler,
    pub(crate) texture_sampler: Sampler,
    pub(crate) global_bind_group: BindGroup,
    pub(crate) light_culling_bind_group: BindGroup,
    pub(crate) forward_bind_group: BindGroup,
    #[cfg(feature = "debug")]
    pub(crate) debug_bind_group: BindGroup,
    pub(crate) screen_size: ScreenSize,
    pub(crate) directional_shadow_size: ScreenSize,
    pub(crate) point_shadow_size: ScreenSize,
    global_uniforms: GlobalUniforms,
    directional_light_uniforms: DirectionalLightUniforms,
    point_light_data: Vec<PointLightData>,
    #[cfg(feature = "debug")]
    debug_uniforms: DebugUniforms,
}

impl Prepare for GlobalContext {
    fn prepare(&mut self, _device: &Device, instructions: &RenderInstruction) {
        self.point_light_data.clear();

        #[allow(unused_mut)]
        let mut ambient_light_color = instructions.uniforms.ambient_light_color;

        #[cfg(feature = "debug")]
        if !instructions.render_settings.show_ambient_light {
            ambient_light_color = Color::BLACK;
        };

        #[allow(unused_mut)]
        let mut directional_light_color = instructions.directional_light_with_shadow.color;

        #[cfg(feature = "debug")]
        if !instructions.render_settings.show_directional_light {
            directional_light_color = Color::BLACK;
        };

        let (indicator_positions, indicator_color) = instructions
            .indicator
            .as_ref()
            .map_or((Matrix4::zero(), Color::WHITE), |indicator| {
                (
                    Matrix4::from_cols(
                        indicator.upper_left.to_homogeneous(),
                        indicator.upper_right.to_homogeneous(),
                        indicator.lower_left.to_homogeneous(),
                        indicator.lower_right.to_homogeneous(),
                    ),
                    indicator.color,
                )
            });

        self.global_uniforms = GlobalUniforms {
            view_projection: (instructions.uniforms.projection_matrix * instructions.uniforms.view_matrix).into(),
            view: instructions.uniforms.view_matrix.into(),
            inverse_view: instructions.uniforms.view_matrix.invert().unwrap_or_else(Matrix4::identity).into(),
            inverse_projection: instructions
                .uniforms
                .projection_matrix
                .invert()
                .unwrap_or_else(Matrix4::identity)
                .into(),
            indicator_positions: indicator_positions.into(),
            indicator_color: indicator_color.components_linear(),
            ambient_color: ambient_light_color.components_linear(),
            screen_size: [self.screen_size.width as u32, self.screen_size.height as u32],
            pointer_position: [instructions.picker_position.left as u32, instructions.picker_position.top as u32],
            animation_timer: instructions.uniforms.animation_timer,
            day_timer: instructions.uniforms.day_timer,
            water_level: instructions.uniforms.water_level,
            point_light_count: (instructions.point_light_shadow_caster.len() + instructions.point_light.len()) as u32,
        };

        self.directional_light_uniforms = DirectionalLightUniforms {
            view_projection: instructions.directional_light_with_shadow.view_projection_matrix.into(),
            color: directional_light_color.components_linear(),
            direction: instructions.directional_light_with_shadow.direction.extend(0.0).into(),
        };

        for (instance_index, instruction) in instructions.point_light_shadow_caster.iter().enumerate() {
            self.point_light_data.push(PointLightData {
                position: instruction.position.to_homogeneous().into(),
                color: instruction.color.components_linear(),
                range: instruction.range,
                texture_index: (instance_index + 1) as i32,
                padding: Default::default(),
            });
        }

        for instruction in instructions.point_light.iter() {
            self.point_light_data.push(PointLightData {
                position: instruction.position.to_homogeneous().into(),
                color: instruction.color.components_linear(),
                range: instruction.range,
                texture_index: 0,
                padding: Default::default(),
            });
        }

        #[cfg(feature = "debug")]
        {
            self.debug_uniforms = DebugUniforms {
                show_picker_buffer: instructions.render_settings.show_picker_buffer as u32,
                show_directional_shadow_map: instructions.render_settings.show_directional_shadow_map as u32,
                show_point_shadow_map: instructions
                    .render_settings
                    .show_point_shadow_map
                    .map(|value| value.get())
                    .unwrap_or(0),
                show_light_culling_count_buffer: instructions.render_settings.show_light_culling_count_buffer as u32,
                show_font_atlas: instructions.render_settings.show_font_atlas as u32,
            };
        }
    }

    fn upload(&mut self, device: &Device, staging_belt: &mut StagingBelt, command_encoder: &mut CommandEncoder) {
        let mut recreated = self
            .global_uniforms_buffer
            .write(device, staging_belt, command_encoder, &[self.global_uniforms]);

        recreated |=
            self.directional_light_uniforms_buffer
                .write(device, staging_belt, command_encoder, &[self.directional_light_uniforms]);

        if !self.point_light_data.is_empty() {
            recreated |= self
                .point_light_data_buffer
                .write(device, staging_belt, command_encoder, &self.point_light_data);
        }

        #[cfg(feature = "debug")]
        {
            recreated |= self
                .debug_uniforms_buffer
                .write(device, staging_belt, command_encoder, &[self.debug_uniforms]);
        }

        if recreated {
            self.global_bind_group = Self::create_global_bind_group(
                device,
                &self.global_uniforms_buffer,
                &self.nearest_sampler,
                &self.linear_sampler,
                &self.texture_sampler,
            );

            self.light_culling_bind_group = Self::create_light_culling_bind_group(
                device,
                &self.point_light_data_buffer,
                &self.tile_light_count_texture,
                &self.tile_light_indices_buffer,
            );

            self.forward_bind_group = Self::create_forward_bind_group(
                device,
                &self.directional_light_uniforms_buffer,
                &self.point_light_data_buffer,
                &self.tile_light_count_texture,
                &self.tile_light_indices_buffer,
                &self.directional_shadow_map_texture,
                &self.point_shadow_map_textures,
            );

            #[cfg(feature = "debug")]
            {
                self.debug_bind_group = Self::create_debug_bind_group(
                    device,
                    &self.debug_uniforms_buffer,
                    &self.picker_buffer_texture,
                    &self.directional_shadow_map_texture,
                    &self.tile_light_count_texture,
                    &self.point_shadow_map_textures,
                );
            }
        }
    }
}

impl GlobalContext {
    fn new(
        device: &Device,
        queue: &Queue,
        texture_loader: &TextureLoader,
        surface_texture_format: TextureFormat,
        msaa: Msaa,
        screen_space_anti_aliasing: ScreenSpaceAntiAliasing,
        screen_size: ScreenSize,
        shadow_detail: ShadowDetail,
        texture_sampler: TextureSamplerType,
    ) -> Self {
        let directional_shadow_size = ScreenSize::uniform(shadow_detail.directional_shadow_resolution() as f32);
        let point_shadow_size = ScreenSize::uniform(shadow_detail.point_shadow_resolution() as f32);

        let solid_pixel_texture = Arc::new(Texture::new_with_data(
            device,
            queue,
            &TextureDescriptor {
                label: Some("solid pixel"),
                size: Extent3d::default(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rgba8Unorm,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: Default::default(),
            },
            &[255, 255, 255, 255],
        ));
        let walk_indicator_texture = texture_loader.get("grid.tga").unwrap();
        let screen_textures = Self::create_screen_size_textures(device, screen_size, msaa, screen_space_anti_aliasing);
        let directional_shadow_map_texture = Self::create_directional_shadow_texture(device, directional_shadow_size);
        let point_shadow_map_textures = Self::create_point_shadow_textures(device, point_shadow_size);
        let resolved_color_texture = Self::create_resolved_color_texture(device, screen_size, msaa, screen_space_anti_aliasing);

        let picker_value_buffer = Buffer::with_capacity(
            device,
            "picker value",
            BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            PickerTarget::value_size() as _,
        );

        let global_uniforms_buffer = Buffer::with_capacity(
            device,
            "global uniforms",
            BufferUsages::COPY_DST | BufferUsages::UNIFORM,
            size_of::<GlobalUniforms>() as _,
        );

        let directional_light_uniforms_buffer = Buffer::with_capacity(
            device,
            "directional light uniforms",
            BufferUsages::COPY_DST | BufferUsages::UNIFORM,
            size_of::<DirectionalLightUniforms>() as _,
        );

        #[cfg(feature = "debug")]
        let debug_uniforms_buffer = Buffer::with_capacity(
            device,
            "debug uniforms",
            BufferUsages::COPY_DST | BufferUsages::UNIFORM,
            size_of::<DebugUniforms>() as _,
        );

        let point_light_data_buffer = Buffer::with_capacity(
            device,
            "point light data",
            BufferUsages::COPY_DST | BufferUsages::STORAGE,
            (128 * size_of::<PointLightData>()) as _,
        );

        let tile_light_indices_buffer = Self::create_tile_light_indices_buffer(device, screen_size);

        let nearest_sampler = create_new_sampler(device, "nearest", TextureSamplerType::Nearest);
        let linear_sampler = create_new_sampler(device, "linear", TextureSamplerType::Linear);
        let texture_sampler = create_new_sampler(device, "texture", texture_sampler);

        let anti_aliasing_resources = Self::create_anti_aliasing_resources(device, screen_space_anti_aliasing, screen_size);

        let global_bind_group = Self::create_global_bind_group(
            device,
            &global_uniforms_buffer,
            &nearest_sampler,
            &linear_sampler,
            &texture_sampler,
        );

        let light_culling_bind_group = Self::create_light_culling_bind_group(
            device,
            &point_light_data_buffer,
            &screen_textures.tile_light_count_texture,
            &tile_light_indices_buffer,
        );

        let forward_bind_group = Self::create_forward_bind_group(
            device,
            &directional_light_uniforms_buffer,
            &point_light_data_buffer,
            &screen_textures.tile_light_count_texture,
            &tile_light_indices_buffer,
            &directional_shadow_map_texture,
            &point_shadow_map_textures,
        );

        #[cfg(feature = "debug")]
        let debug_bind_group = Self::create_debug_bind_group(
            device,
            &debug_uniforms_buffer,
            &screen_textures.picker_buffer_texture,
            &directional_shadow_map_texture,
            &screen_textures.tile_light_count_texture,
            &point_shadow_map_textures,
        );

        Self {
            surface_texture_format,
            msaa,
            screen_space_anti_aliasing,
            solid_pixel_texture,
            walk_indicator_texture,
            forward_depth_texture: screen_textures.forward_depth_texture,
            picker_buffer_texture: screen_textures.picker_buffer_texture,
            picker_depth_texture: screen_textures.picker_depth_texture,
            forward_color_texture: screen_textures.forward_color_texture,
            resolved_color_texture,
            interface_buffer_texture: screen_textures.interface_buffer_texture,
            directional_shadow_map_texture,
            point_shadow_map_textures,
            tile_light_count_texture: screen_textures.tile_light_count_texture,
            global_uniforms_buffer,
            forward_bind_group,
            #[cfg(feature = "debug")]
            debug_bind_group,
            directional_light_uniforms_buffer,
            tile_light_indices_buffer,
            #[cfg(feature = "debug")]
            debug_uniforms_buffer,
            picker_value_buffer,
            point_light_data_buffer,
            anti_aliasing_resources,
            nearest_sampler,
            linear_sampler,
            texture_sampler,
            global_bind_group,
            light_culling_bind_group,
            screen_size,
            directional_shadow_size,
            point_shadow_size,
            global_uniforms: GlobalUniforms::default(),
            directional_light_uniforms: DirectionalLightUniforms::default(),
            point_light_data: Vec::default(),
            #[cfg(feature = "debug")]
            debug_uniforms: DebugUniforms::default(),
        }
    }

    fn get_color_texture(&self) -> &AttachmentTexture {
        self.resolved_color_texture.as_ref().unwrap_or(&self.forward_color_texture)
    }

    fn create_screen_size_textures(
        device: &Device,
        screen_size: ScreenSize,
        msaa: Msaa,
        screen_space_anti_aliasing: ScreenSpaceAntiAliasing,
    ) -> ScreenSizeTextures {
        // Since we need to copy from the picker attachment to read the picker value, we
        // need to align both attachments properly to the requirements of
        // COPY_BYTES_PER_ROW_ALIGNMENT.
        let block_size = TextureFormat::Rg32Uint.block_copy_size(None).unwrap();
        let picker_padded_width = ((screen_size.width as u32 * block_size + (COPY_BYTES_PER_ROW_ALIGNMENT - 1))
            & !(COPY_BYTES_PER_ROW_ALIGNMENT - 1))
            / block_size;

        let picker_factory = AttachmentTextureFactory::new(device, screen_size, 1, Some(picker_padded_width));
        let picker_buffer_texture = picker_factory.new_attachment(
            "picker buffer",
            TextureFormat::Rg32Uint,
            AttachmentTextureType::PickerAttachment,
        );
        let picker_depth_texture = picker_factory.new_attachment("depth", TextureFormat::Depth32Float, AttachmentTextureType::Depth);

        let (forward_color_texture, forward_depth_texture) =
            Self::create_forward_texture(device, screen_size, msaa, screen_space_anti_aliasing);

        let interface_screen_factory = AttachmentTextureFactory::new(device, screen_size, 4, None);

        let interface_buffer_texture = interface_screen_factory.new_attachment(
            "interface buffer",
            INTERFACE_TEXTURE_FORMAT,
            AttachmentTextureType::ColorAttachment,
        );

        let (tile_x, tile_y) = calculate_light_tile_count(screen_size);

        let tile_light_count_texture = StorageTexture::new(device, "tile light count texture", tile_x, tile_y, TextureFormat::R32Uint);

        ScreenSizeTextures {
            forward_depth_texture,
            picker_buffer_texture,
            picker_depth_texture,
            forward_color_texture,
            interface_buffer_texture,
            tile_light_count_texture,
        }
    }

    fn create_resolved_color_texture(
        device: &Device,
        screen_size: ScreenSize,
        msaa: Msaa,
        screen_space_anti_aliasing: ScreenSpaceAntiAliasing,
    ) -> Option<AttachmentTexture> {
        let need_texture = msaa.multisampling_activated();
        let attachment_type = match screen_space_anti_aliasing == ScreenSpaceAntiAliasing::Cmaa2 {
            true => AttachmentTextureType::ColorStorageAttachment,
            false => AttachmentTextureType::ColorAttachment,
        };

        match need_texture {
            true => {
                let attachment_factory = AttachmentTextureFactory::new(device, screen_size, 1, None);
                Some(attachment_factory.new_attachment("resolved color", RENDER_TO_TEXTURE_FORMAT, attachment_type))
            }
            false => None,
        }
    }

    fn create_forward_texture(
        device: &Device,
        screen_size: ScreenSize,
        msaa: Msaa,
        screen_space_anti_aliasing: ScreenSpaceAntiAliasing,
    ) -> (AttachmentTexture, AttachmentTexture) {
        let texture_type = match !msaa.multisampling_activated() && screen_space_anti_aliasing == ScreenSpaceAntiAliasing::Cmaa2 {
            true => AttachmentTextureType::ColorStorageAttachment,
            false => AttachmentTextureType::ColorAttachment,
        };

        let factory = AttachmentTextureFactory::new(device, screen_size, msaa.sample_count(), None);
        let color_texture = factory.new_attachment("forward color", RENDER_TO_TEXTURE_FORMAT, texture_type);
        let depth_texture = factory.new_attachment("forward depth", TextureFormat::Depth32Float, AttachmentTextureType::Depth);
        (color_texture, depth_texture)
    }

    fn create_directional_shadow_texture(device: &Device, shadow_size: ScreenSize) -> AttachmentTexture {
        let shadow_factory = AttachmentTextureFactory::new(device, shadow_size, 1, None);

        shadow_factory.new_attachment(
            "directional shadow map",
            TextureFormat::Depth32Float,
            AttachmentTextureType::DepthAttachment,
        )
    }

    fn create_tile_light_indices_buffer(device: &Device, screen_size: ScreenSize) -> Buffer<TileLightIndices> {
        let (tile_count_x, tile_count_y) = calculate_light_tile_count(screen_size);

        Buffer::with_capacity(
            device,
            "tile light indices",
            BufferUsages::STORAGE,
            ((tile_count_x * tile_count_y).max(1) as usize * size_of::<TileLightIndices>()) as _,
        )
    }

    fn create_point_shadow_textures(device: &Device, shadow_size: ScreenSize) -> CubeArrayTexture {
        CubeArrayTexture::new(
            device,
            "point shadow map",
            shadow_size,
            TextureFormat::Depth32Float,
            AttachmentTextureType::DepthAttachment,
            NUMBER_OF_POINT_LIGHTS_WITH_SHADOWS as u32,
        )
    }

    fn create_anti_aliasing_resources(
        device: &Device,
        screen_space_anti_aliasing: ScreenSpaceAntiAliasing,
        screen_size: ScreenSize,
    ) -> AntiAliasingResource {
        match screen_space_anti_aliasing {
            ScreenSpaceAntiAliasing::Off => AntiAliasingResource::None,
            ScreenSpaceAntiAliasing::Fxaa => {
                let factory = AttachmentTextureFactory::new(device, screen_size, 1, None);
                let color_with_luma_texture = factory.new_attachment(
                    "fxaa2 color with luma",
                    FXAA_COLOR_LUMA_TEXTURE_FORMAT,
                    AttachmentTextureType::ColorAttachment,
                );
                let resources = FxaaResources { color_with_luma_texture };
                AntiAliasingResource::Fxaa(Box::new(resources))
            }
            ScreenSpaceAntiAliasing::Cmaa2 => {
                let width = screen_size.width as usize;
                let height = screen_size.height as usize;

                let max_shape_candidates = width * height / 4;
                let max_deferred_blend_items = width * height / 2;
                let max_deferred_blend_locations = (width * height + 3) / 6;

                let edges_textures = StorageTexture::new(
                    device,
                    "cmaa2 edges",
                    (width as u32 + 1) / 2,
                    height as u32,
                    TextureFormat::R8Uint,
                );
                let control_buffer = Buffer::with_capacity(device, "cmaa2 control", BufferUsages::STORAGE, (4 * size_of::<u32>()) as _);
                let indirect_buffer = Buffer::with_capacity(
                    device,
                    "cmaa2 indirect",
                    BufferUsages::STORAGE | BufferUsages::INDIRECT,
                    size_of::<DispatchIndirectArgs>() as _,
                ) as _;
                let shape_candidates_buffer = Buffer::with_capacity(
                    device,
                    "cmaa2 candidates",
                    BufferUsages::STORAGE,
                    ((max_shape_candidates.max(1) * size_of::<u32>()) as u64).min(MAX_BUFFER_SIZE),
                );
                let deferred_blend_item_list_heads_buffer = Buffer::with_capacity(
                    device,
                    "cmaa2 deferred blend item list heads",
                    BufferUsages::STORAGE,
                    (((((width + 1) / 2) * ((height + 1) / 2)).max(1) * size_of::<u32>()) as u64).min(MAX_BUFFER_SIZE),
                );
                let deferred_blend_item_list_buffer = Buffer::with_capacity(
                    device,
                    "cmaa2 deferred blend item list",
                    BufferUsages::STORAGE,
                    ((max_deferred_blend_items.max(1) * size_of::<[u32; 2]>()) as u64).min(MAX_BUFFER_SIZE),
                );
                let deferred_blend_location_list_buffer = Buffer::with_capacity(
                    device,
                    "cmaa2 deferred blend location list",
                    BufferUsages::STORAGE,
                    ((max_deferred_blend_locations.max(1) * size_of::<u32>()) as u64).min(MAX_BUFFER_SIZE),
                );

                let bind_group = Self::create_cmaa2_bind_group(
                    device,
                    &edges_textures,
                    &control_buffer,
                    &shape_candidates_buffer,
                    &indirect_buffer,
                    &deferred_blend_item_list_heads_buffer,
                    &deferred_blend_item_list_buffer,
                    &deferred_blend_location_list_buffer,
                );

                let resources = Cmaa2Resources {
                    _edges_textures: edges_textures,
                    _control_buffer: control_buffer,
                    _shape_candidates_buffer: shape_candidates_buffer,
                    indirect_buffer,
                    _deferred_blend_item_list_heads_buffer: deferred_blend_item_list_heads_buffer,
                    _deferred_blend_item_list_buffer: deferred_blend_item_list_buffer,
                    _deferred_blend_location_list_buffer: deferred_blend_location_list_buffer,
                    bind_group,
                };
                AntiAliasingResource::Cmaa2(Box::new(resources))
            }
        }
    }

    fn update_screen_size_resources(&mut self, device: &Device, screen_size: ScreenSize) {
        self.screen_size = screen_size;
        let ScreenSizeTextures {
            forward_color_texture,
            forward_depth_texture,
            picker_buffer_texture,
            picker_depth_texture,
            interface_buffer_texture,
            tile_light_count_texture,
        } = Self::create_screen_size_textures(device, self.screen_size, self.msaa, self.screen_space_anti_aliasing);

        let resolved_color_texture =
            Self::create_resolved_color_texture(device, self.screen_size, self.msaa, self.screen_space_anti_aliasing);

        self.forward_color_texture = forward_color_texture;
        self.forward_depth_texture = forward_depth_texture;
        self.picker_buffer_texture = picker_buffer_texture;
        self.picker_depth_texture = picker_depth_texture;
        self.resolved_color_texture = resolved_color_texture;
        self.interface_buffer_texture = interface_buffer_texture;
        self.tile_light_count_texture = tile_light_count_texture;

        self.tile_light_indices_buffer = Self::create_tile_light_indices_buffer(device, screen_size);

        self.anti_aliasing_resources = Self::create_anti_aliasing_resources(device, self.screen_space_anti_aliasing, self.screen_size);

        // We need to update this bind group, because it's content changed, and it isn't
        // re-created each frame.
        self.light_culling_bind_group = Self::create_light_culling_bind_group(
            device,
            &self.point_light_data_buffer,
            &self.tile_light_count_texture,
            &self.tile_light_indices_buffer,
        );

        self.forward_bind_group = Self::create_forward_bind_group(
            device,
            &self.directional_light_uniforms_buffer,
            &self.point_light_data_buffer,
            &self.tile_light_count_texture,
            &self.tile_light_indices_buffer,
            &self.directional_shadow_map_texture,
            &self.point_shadow_map_textures,
        );

        #[cfg(feature = "debug")]
        {
            self.debug_bind_group = Self::create_debug_bind_group(
                device,
                &self.debug_uniforms_buffer,
                &self.picker_buffer_texture,
                &self.directional_shadow_map_texture,
                &self.tile_light_count_texture,
                &self.point_shadow_map_textures,
            );
        }
    }

    fn update_shadow_size_textures(&mut self, device: &Device, shadow_detail: ShadowDetail) {
        self.directional_shadow_size = ScreenSize::uniform(shadow_detail.directional_shadow_resolution() as f32);
        self.point_shadow_size = ScreenSize::uniform(shadow_detail.point_shadow_resolution() as f32);

        self.directional_shadow_map_texture = Self::create_directional_shadow_texture(device, self.directional_shadow_size);
        self.point_shadow_map_textures = Self::create_point_shadow_textures(device, self.point_shadow_size);

        // We need to update this bind group, because it's content changed, and it isn't
        // re-created each frame.
        self.forward_bind_group = Self::create_forward_bind_group(
            device,
            &self.directional_light_uniforms_buffer,
            &self.point_light_data_buffer,
            &self.tile_light_count_texture,
            &self.tile_light_indices_buffer,
            &self.directional_shadow_map_texture,
            &self.point_shadow_map_textures,
        );

        #[cfg(feature = "debug")]
        {
            self.debug_bind_group = Self::create_debug_bind_group(
                device,
                &self.debug_uniforms_buffer,
                &self.picker_buffer_texture,
                &self.directional_shadow_map_texture,
                &self.tile_light_count_texture,
                &self.point_shadow_map_textures,
            );
        }
    }

    fn update_texture_sampler(&mut self, device: &Device, texture_sampler_type: TextureSamplerType) {
        self.texture_sampler = create_new_sampler(device, "texture", texture_sampler_type);
        self.global_bind_group = Self::create_global_bind_group(
            device,
            &self.global_uniforms_buffer,
            &self.nearest_sampler,
            &self.linear_sampler,
            &self.texture_sampler,
        );
    }

    fn update_msaa(&mut self, device: &Device, msaa: Msaa) {
        self.msaa = msaa;

        (self.forward_color_texture, self.forward_depth_texture) =
            Self::create_forward_texture(device, self.screen_size, self.msaa, self.screen_space_anti_aliasing);

        self.resolved_color_texture =
            Self::create_resolved_color_texture(device, self.screen_size, self.msaa, self.screen_space_anti_aliasing);
    }

    fn update_screen_space_anti_aliasing(&mut self, device: &Device, screen_space_anti_aliasing: ScreenSpaceAntiAliasing) {
        self.screen_space_anti_aliasing = screen_space_anti_aliasing;

        (self.forward_color_texture, self.forward_depth_texture) =
            Self::create_forward_texture(device, self.screen_size, self.msaa, self.screen_space_anti_aliasing);

        self.resolved_color_texture =
            Self::create_resolved_color_texture(device, self.screen_size, self.msaa, self.screen_space_anti_aliasing);

        self.anti_aliasing_resources = Self::create_anti_aliasing_resources(device, self.screen_space_anti_aliasing, self.screen_size);
    }

    fn global_bind_group_layout(device: &Device) -> &'static BindGroupLayout {
        static LAYOUT: OnceLock<BindGroupLayout> = OnceLock::new();
        LAYOUT.get_or_init(|| {
            device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("global"),
                entries: &[
                    BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ShaderStages::all(),
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(size_of::<GlobalUniforms>() as _),
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 1,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Sampler(SamplerBindingType::Filtering),
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 2,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Sampler(SamplerBindingType::Filtering),
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 3,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Sampler(SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            })
        })
    }

    fn light_culling_bind_group_layout(device: &Device) -> &'static BindGroupLayout {
        static LAYOUT: OnceLock<BindGroupLayout> = OnceLock::new();
        LAYOUT.get_or_init(|| {
            device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("light culling"),
                entries: &[
                    BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ShaderStages::COMPUTE,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 1,
                        visibility: ShaderStages::COMPUTE,
                        ty: BindingType::StorageTexture {
                            access: StorageTextureAccess::WriteOnly,
                            format: TextureFormat::R32Uint,
                            view_dimension: TextureViewDimension::D2,
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 2,
                        visibility: ShaderStages::COMPUTE,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(size_of::<TileLightIndices>() as _),
                        },
                        count: None,
                    },
                ],
            })
        })
    }

    fn forward_bind_group_layout(device: &Device) -> &'static BindGroupLayout {
        static LAYOUT: OnceLock<BindGroupLayout> = OnceLock::new();
        LAYOUT.get_or_init(|| {
            device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("forward"),
                entries: &[
                    BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ShaderStages::all(),
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(size_of::<DirectionalLightUniforms>() as _),
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 1,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Texture {
                            sample_type: TextureSampleType::Depth,
                            view_dimension: TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 2,
                        visibility: ShaderStages::VERTEX_FRAGMENT,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 3,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Texture {
                            sample_type: TextureSampleType::Uint,
                            view_dimension: TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 4,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(size_of::<TileLightIndices>() as _),
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 5,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Texture {
                            sample_type: TextureSampleType::Depth,
                            view_dimension: TextureViewDimension::CubeArray,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            })
        })
    }

    fn cmaa2_bind_group_layout(device: &Device) -> &'static BindGroupLayout {
        static LAYOUT: OnceLock<BindGroupLayout> = OnceLock::new();
        LAYOUT.get_or_init(|| {
            device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("cmaa2"),
                entries: &[
                    BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ShaderStages::COMPUTE,
                        ty: BindingType::StorageTexture {
                            access: StorageTextureAccess::ReadWrite,
                            format: TextureFormat::R8Uint,
                            view_dimension: TextureViewDimension::D2,
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 1,
                        visibility: ShaderStages::COMPUTE,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(size_of::<u32>() as _),
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 2,
                        visibility: ShaderStages::COMPUTE,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(size_of::<u32>() as _),
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 3,
                        visibility: ShaderStages::COMPUTE,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(size_of::<DispatchIndirectArgs>() as _),
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 4,
                        visibility: ShaderStages::COMPUTE,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(size_of::<u32>() as _),
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 5,
                        visibility: ShaderStages::COMPUTE,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(size_of::<[u32; 2]>() as _),
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 6,
                        visibility: ShaderStages::COMPUTE,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(size_of::<u32>() as _),
                        },
                        count: None,
                    },
                ],
            })
        })
    }

    fn cmaa2_output_bind_group_layout(device: &Device) -> &'static BindGroupLayout {
        static LAYOUT: OnceLock<BindGroupLayout> = OnceLock::new();
        LAYOUT.get_or_init(|| {
            device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("cmaa2 output"),
                entries: &[BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::StorageTexture {
                        access: StorageTextureAccess::WriteOnly,
                        format: RENDER_TO_TEXTURE_FORMAT,
                        view_dimension: TextureViewDimension::D2,
                    },
                    count: None,
                }],
            })
        })
    }

    #[cfg(feature = "debug")]
    fn debug_bind_group_layout(device: &Device) -> &'static BindGroupLayout {
        static LAYOUT: OnceLock<BindGroupLayout> = OnceLock::new();
        LAYOUT.get_or_init(|| {
            device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("debug"),
                entries: &[
                    BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ShaderStages::all(),
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(size_of::<DebugUniforms>() as _),
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 1,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Texture {
                            sample_type: TextureSampleType::Uint,
                            view_dimension: TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 2,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Texture {
                            sample_type: TextureSampleType::Depth,
                            view_dimension: TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 3,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Texture {
                            sample_type: TextureSampleType::Uint,
                            view_dimension: TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 4,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Texture {
                            sample_type: TextureSampleType::Depth,
                            view_dimension: TextureViewDimension::CubeArray,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            })
        })
    }

    fn create_global_bind_group(
        device: &Device,
        global_uniforms_buffer: &Buffer<GlobalUniforms>,
        nearest_sampler: &Sampler,
        linear_sampler: &Sampler,
        texture_sampler: &Sampler,
    ) -> BindGroup {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some("global"),
            layout: Self::global_bind_group_layout(device),
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: global_uniforms_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(nearest_sampler),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Sampler(linear_sampler),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: BindingResource::Sampler(texture_sampler),
                },
            ],
        })
    }

    fn create_light_culling_bind_group(
        device: &Device,
        point_light_data_buffer: &Buffer<PointLightData>,
        tile_light_count_texture: &StorageTexture,
        tile_light_indices_buffer: &Buffer<TileLightIndices>,
    ) -> BindGroup {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some("light culling"),
            layout: Self::light_culling_bind_group_layout(device),
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: point_light_data_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(tile_light_count_texture.get_texture_view()),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: tile_light_indices_buffer.as_entire_binding(),
                },
            ],
        })
    }

    fn create_forward_bind_group(
        device: &Device,
        directional_light_uniforms_buffer: &Buffer<DirectionalLightUniforms>,
        point_light_data_buffer: &Buffer<PointLightData>,
        tile_light_count_texture: &StorageTexture,
        tile_light_indices_buffer: &Buffer<TileLightIndices>,
        directional_shadow_map_texture: &AttachmentTexture,
        point_shadow_maps_texture: &CubeArrayTexture,
    ) -> BindGroup {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some("forward"),
            layout: Self::forward_bind_group_layout(device),
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: directional_light_uniforms_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(directional_shadow_map_texture.get_texture_view()),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: point_light_data_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: BindingResource::TextureView(tile_light_count_texture.get_texture_view()),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: tile_light_indices_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 5,
                    resource: BindingResource::TextureView(point_shadow_maps_texture.get_texture_view()),
                },
            ],
        })
    }

    fn create_cmaa2_bind_group(
        device: &Device,
        edges_textures: &StorageTexture,
        control_buffer: &Buffer<u32>,
        shape_candidates_buffer: &Buffer<u32>,
        indirect_buffer: &Buffer<DispatchIndirectArgs>,
        deferred_blend_item_list_heads_buffer: &Buffer<u32>,
        deferred_blend_item_list_buffer: &Buffer<[u32; 2]>,
        deferred_blend_location_list_buffer: &Buffer<u32>,
    ) -> BindGroup {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some("cmaa2"),
            layout: Self::cmaa2_bind_group_layout(device),
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(edges_textures.get_texture_view()),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: control_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: shape_candidates_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: indirect_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: deferred_blend_item_list_heads_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 5,
                    resource: deferred_blend_item_list_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 6,
                    resource: deferred_blend_location_list_buffer.as_entire_binding(),
                },
            ],
        })
    }

    fn create_cmaa2_output_bind_group(device: &Device, output_color_texture: &AttachmentTexture) -> BindGroup {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some("cmaa2 output"),
            layout: Self::cmaa2_output_bind_group_layout(device),
            entries: &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(output_color_texture.get_texture_view()),
            }],
        })
    }

    #[cfg(feature = "debug")]
    fn create_debug_bind_group(
        device: &Device,
        debug_uniforms_buffer: &Buffer<DebugUniforms>,
        picker_buffer_texture: &AttachmentTexture,
        directional_shadow_map_texture: &AttachmentTexture,
        tile_light_count_texture: &StorageTexture,
        point_shadow_maps_texture: &CubeArrayTexture,
    ) -> BindGroup {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some("debug"),
            layout: Self::debug_bind_group_layout(device),
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: debug_uniforms_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(picker_buffer_texture.get_texture_view()),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::TextureView(directional_shadow_map_texture.get_texture_view()),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: BindingResource::TextureView(tile_light_count_texture.get_texture_view()),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: BindingResource::TextureView(point_shadow_maps_texture.get_texture_view()),
                },
            ],
        })
    }
}

fn calculate_light_tile_count(screen_size: ScreenSize) -> (u32, u32) {
    let tile_count_x = (screen_size.width as u32 + LIGHT_TILE_SIZE - 1) / LIGHT_TILE_SIZE;
    let tile_count_y = (screen_size.height as u32 + LIGHT_TILE_SIZE - 1) / LIGHT_TILE_SIZE;
    (tile_count_x, tile_count_y)
}

struct ScreenSizeTextures {
    forward_color_texture: AttachmentTexture,
    forward_depth_texture: AttachmentTexture,
    picker_buffer_texture: AttachmentTexture,
    picker_depth_texture: AttachmentTexture,
    interface_buffer_texture: AttachmentTexture,
    tile_light_count_texture: StorageTexture,
}

pub(crate) enum AntiAliasingResource {
    None,
    Fxaa(Box<FxaaResources>),
    Cmaa2(Box<Cmaa2Resources>),
}

pub(crate) struct FxaaResources {
    color_with_luma_texture: AttachmentTexture,
}

pub(crate) struct Cmaa2Resources {
    _edges_textures: StorageTexture,
    /// Holds three values used for atomic counters:
    ///  - 0: Shape candidate count
    ///  - 1: Blend location count
    ///  - 2: Deferred blend item count
    _control_buffer: Buffer<u32>,
    _shape_candidates_buffer: Buffer<u32>,
    indirect_buffer: Buffer<DispatchIndirectArgs>,
    _deferred_blend_item_list_heads_buffer: Buffer<u32>,
    _deferred_blend_item_list_buffer: Buffer<[u32; 2]>,
    _deferred_blend_location_list_buffer: Buffer<u32>,
    bind_group: BindGroup,
}
