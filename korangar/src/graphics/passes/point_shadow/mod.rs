mod entity;
mod indicator;
mod model;

use std::sync::OnceLock;

use bytemuck::{bytes_of, Pod, Zeroable};
pub(crate) use entity::PointShadowEntityDrawer;
pub(crate) use indicator::PointShadowIndicatorDrawer;
pub(crate) use model::PointShadowModelDrawer;
use wgpu::util::StagingBelt;
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource,
    BindingType, BufferBindingType, BufferUsages, CommandEncoder, Device, LoadOp, Operations, Queue, RenderPass,
    RenderPassDepthStencilAttachment, RenderPassDescriptor, ShaderStages, StoreOp, TextureFormat, TextureView,
};

use super::{BindGroupCount, ColorAttachmentCount, DepthAttachmentCount, RenderPassContext};
use crate::graphics::{Buffer, GlobalContext, ModelVertex, PointShadowCasterInstruction, Prepare, RenderInstruction, TextureGroup};
use crate::loaders::TextureLoader;
use crate::NUMBER_OF_POINT_LIGHTS_WITH_SHADOWS;

const PASS_NAME: &str = "point shadow render pass";
const NUMBER_FACES: usize = 6;

#[derive(Copy, Clone, Pod, Zeroable)]
#[repr(C)]
struct PassUniforms {
    view_projection: [[f32; 4]; 4],
    light_position: [f32; 4],
    animation_timer: f32,
    padding: [u32; 3],
}

#[derive(Copy, Clone)]
pub(crate) struct PointShadowData {
    pub(crate) shadow_caster_index: usize,
    pub(crate) face_index: usize,
}

pub(crate) struct PointShadowBatchData<'a> {
    pub(crate) pass_data: PointShadowData,
    pub(crate) caster: &'a [PointShadowCasterInstruction],
    pub(crate) map_textures: &'a TextureGroup,
    pub(crate) map_vertex_group: &'a Buffer<ModelVertex>,
}

pub(crate) struct PointShadowRenderPassContext {
    point_shadow_texture_format: TextureFormat,
    uniforms_buffer: Buffer<u8>,
    bind_group: BindGroup,
    uniforms_data: Vec<PassUniforms>,
    buffer_data: Box<[u8]>,
    aligned_size: usize,
}

impl RenderPassContext<{ BindGroupCount::Two }, { ColorAttachmentCount::None }, { DepthAttachmentCount::One }>
    for PointShadowRenderPassContext
{
    type PassData<'data> = PointShadowData;

    fn new(device: &Device, _queue: &Queue, _texture_loader: &TextureLoader, global_context: &GlobalContext) -> Self {
        let point_shadow_texture_format = global_context.point_shadow_map_textures[0].get_texture_format();

        let uniform_alignment = device.limits().min_uniform_buffer_offset_alignment as usize;
        let aligned_size = (size_of::<PassUniforms>() + uniform_alignment - 1) & !(uniform_alignment - 1);
        let buffer_size = aligned_size * NUMBER_OF_POINT_LIGHTS_WITH_SHADOWS * NUMBER_FACES;

        let uniforms_buffer = Buffer::with_capacity(
            device,
            format!("{PASS_NAME} pass uniforms"),
            BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            buffer_size as _,
        );

        let bind_group = Self::create_bind_group(device, &uniforms_buffer);
        let uniforms_data = Vec::with_capacity(NUMBER_OF_POINT_LIGHTS_WITH_SHADOWS * NUMBER_FACES);
        let buffer_data = vec![0; buffer_size].into_boxed_slice();

        Self {
            point_shadow_texture_format,
            uniforms_buffer,
            bind_group,
            uniforms_data,
            buffer_data,
            aligned_size,
        }
    }

    fn create_pass<'encoder>(
        &mut self,
        _frame_view: &TextureView,
        encoder: &'encoder mut CommandEncoder,
        global_context: &GlobalContext,
        pass_data: PointShadowData,
    ) -> RenderPass<'encoder> {
        let dynamic_offset = ((pass_data.shadow_caster_index * NUMBER_FACES + pass_data.face_index) * self.aligned_size) as u32;

        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some(PASS_NAME),
            color_attachments: &[],
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: global_context.point_shadow_map_textures[pass_data.shadow_caster_index].get_texture_face_view(pass_data.face_index),
                depth_ops: Some(Operations {
                    load: LoadOp::Clear(1.0),
                    store: StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_viewport(
            0.0,
            0.0,
            global_context.point_shadow_size.width,
            global_context.point_shadow_size.height,
            0.0,
            1.0,
        );
        pass.set_bind_group(0, &global_context.global_bind_group, &[]);
        pass.set_bind_group(1, &self.bind_group, &[dynamic_offset]);

        pass
    }

    fn bind_group_layout(device: &Device) -> [&'static BindGroupLayout; 2] {
        static LAYOUT: OnceLock<BindGroupLayout> = OnceLock::new();

        let layout = LAYOUT.get_or_init(|| {
            device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some(PASS_NAME),
                entries: &[BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::VERTEX_FRAGMENT,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: std::num::NonZeroU64::new(size_of::<PassUniforms>() as _),
                    },
                    count: None,
                }],
            })
        });

        [GlobalContext::global_bind_group_layout(device), layout]
    }

    fn color_attachment_formats(&self) -> [TextureFormat; 0] {
        []
    }

    fn depth_attachment_output_format(&self) -> [TextureFormat; 1] {
        [self.point_shadow_texture_format]
    }
}

impl Prepare for PointShadowRenderPassContext {
    fn prepare(&mut self, _device: &Device, instructions: &RenderInstruction) {
        self.uniforms_data.clear();
        instructions.point_light_shadow_caster.iter().for_each(|caster| {
            (0..NUMBER_FACES).for_each(|face_index| {
                let uniform = PassUniforms {
                    view_projection: caster.view_projection_matrices[face_index].into(),
                    light_position: caster.position.to_homogeneous().into(),
                    animation_timer: instructions.uniforms.animation_timer,
                    padding: Default::default(),
                };
                self.uniforms_data.push(uniform);
            });
        });

        for (index, uniform) in self.uniforms_data.iter().enumerate() {
            let start = index * self.aligned_size;
            let end = start + size_of::<PassUniforms>();
            self.buffer_data[start..end].copy_from_slice(bytes_of(uniform));
        }
    }

    fn upload(&mut self, device: &Device, staging_belt: &mut StagingBelt, command_encoder: &mut CommandEncoder) {
        let recreated = self.uniforms_buffer.write(device, staging_belt, command_encoder, &self.buffer_data);

        if recreated {
            self.bind_group = Self::create_bind_group(device, &self.uniforms_buffer);
        }
    }
}

impl PointShadowRenderPassContext {
    fn create_bind_group(device: &Device, uniforms_buffer: &Buffer<u8>) -> BindGroup {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some(PASS_NAME),
            layout: Self::bind_group_layout(device)[1],
            entries: &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: uniforms_buffer.get_buffer(),
                    offset: 0,
                    size: Some(std::num::NonZeroU64::new(size_of::<PassUniforms>() as u64).unwrap()),
                }),
            }],
        })
    }
}