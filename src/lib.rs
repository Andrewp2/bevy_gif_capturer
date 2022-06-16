use bevy::{
    prelude::{Commands, EventReader, Plugin, Res, ResMut, SystemStage, Time, Timer, World},
    render::{
        render_graph::{self, Node, RenderGraph},
        render_resource::{
            Buffer, BufferDescriptor, BufferUsages, Extent3d, ImageCopyBuffer, ImageCopyTexture,
            ImageDataLayout, MapMode, Origin3d, TextureAspect,
        },
        renderer::{RenderContext, RenderDevice},
        view::WindowSurfaces,
        RenderApp, RenderStage,
    },
    window::{Window, Windows},
};
use gif::Repeat;
use std::{mem, num::NonZeroU32, path::Path, time::Duration};

#[derive(Clone)]
pub struct GifCaptureSettings {
    pub duration: f32,
    pub path: &'static str,
    pub repeat: Repeat,
    pub speed: i32,
    _private: (),
}

impl Default for GifCaptureSettings {
    fn default() -> Self {
        GifCaptureSettings {
            duration: 5.0,
            path: "",
            repeat: Repeat::Infinite,
            speed: 10,
            _private: (),
        }
    }
}

pub struct GifCaptureSettingsError {
    pub reason: String,
}

impl GifCaptureSettings {
    /// Creates a new GifCaptureSettings. Returns an error for bad options passed.
    pub fn new(
        duration: f32,
        path: &'static str,
        repeat: Repeat,
        speed: i32,
    ) -> Result<GifCaptureSettings, GifCaptureSettingsError> {
        if !Path::exists(Path::new(path)) {
            return Err(GifCaptureSettingsError {
                reason: format!("Path: {} doesn't exist.", path),
            });
        }
        if speed < 1 || speed > 30 {
            return Err(GifCaptureSettingsError {
                reason: format!("Speed: {} must be within range of 1 to 30, see: https://docs.rs/gif/0.11.3/gif/struct.Frame.html#method.from_rgba_speed", speed),
            });
        }
        return Ok(GifCaptureSettings {
            duration,
            path,
            repeat,
            speed,
            _private: (),
        });
    }
}

/// Extracts the settings from the App world to the Render world.
fn extract_settings(mut commands: Commands, settings: Option<Res<GifCaptureSettings>>) {
    if let Some(settings) = settings {
        commands.insert_resource(settings.clone());
    }
}

fn extract_gif_capture(
    mut commands: Commands,
    event: EventReader<GifCaptureStartEvent>,
    mut gif_time: ResMut<GifTime>,
    gif_settings: Res<GifCaptureSettings>,
) {
    gif_time
        .timer
        .set_duration(Duration::from_secs_f32(gif_settings.duration));
    if !event.is_empty() {
        commands.insert_resource(GifCaptureState::CurrentlyCapturing);
        // Resets it, notably it resets it in the App world.
        gif_time.timer.reset();
    }
    if gif_time.timer.just_finished() {
        commands.insert_resource(GifCaptureState::JustFinishedCapturing);
    }
}

#[derive(Default)]
struct GifTime {
    timer: Timer,
}

enum GifCaptureState {
    Off,
    CurrentlyCapturing,
    JustFinishedCapturing,
}

impl Default for GifCaptureState {
    fn default() -> Self {
        GifCaptureState::Off
    }
}

struct DispatchGifCapture;

/// Node for dispatching the gif capture in the RenderGraph.
/// Copies the texture from the primary window surface, back to the buffer we created earlier.
impl Node for DispatchGifCapture {
    fn run(
        &self,
        _graph: &mut render_graph::RenderGraphContext,
        render_context: &mut RenderContext,
        world: &World,
    ) -> Result<(), render_graph::NodeRunError> {
        if let Some(gif_state) = world.get_resource::<GifCaptureState>() {
            if let GifCaptureState::CurrentlyCapturing = gif_state {
                let windows = world.get_resource::<Windows>().unwrap();
                let window_surfaces = world.get_resource::<WindowSurfaces>().unwrap();
                let command_encoder = world.get_resource::<RenderContext>().unwrap();
                let output_buffer = world.get_resource::<GifBuffer>();
                let primary_window = windows.primary();
                let surface = window_surfaces.surfaces.get(&primary_window.id());
                if let (Some(surface), Some(output_buffer)) = (surface, output_buffer) {
                    let surface_texture = surface.get_current_texture().unwrap();
                    let (_, padded_bytes_per_row, _) = get_buffer_size(&primary_window);
                    render_context.command_encoder.copy_texture_to_buffer(
                        ImageCopyTexture {
                            texture: &surface_texture.texture,
                            mip_level: 0,
                            origin: Origin3d::ZERO,
                            aspect: TextureAspect::All,
                        },
                        ImageCopyBuffer {
                            buffer: &output_buffer.0,
                            layout: ImageDataLayout {
                                offset: 0,
                                bytes_per_row: NonZeroU32::new(padded_bytes_per_row as u32),
                                rows_per_image: NonZeroU32::new(primary_window.height() as u32),
                            },
                        },
                        Extent3d {
                            width: (primary_window.width() as u32),
                            height: (primary_window.height() as u32),
                            depth_or_array_layers: 0u32,
                        },
                    );
                }
            }
        }
        Ok(())
    }
}

pub struct GifCaptureStartEvent;
pub struct GifCapturePlugin;

#[derive(Default)]
pub struct GifCaptureFrames(Vec<Vec<u8>>);

fn read_capture_events_and_tick_timer(mut gif_time: ResMut<GifTime>, time: Res<Time>) {
    gif_time.timer.tick(time.delta());
    if gif_time.timer.just_finished() {}
}

/// Core plugin for capturing gifs.
impl Plugin for GifCapturePlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        app.init_resource::<GifTime>();
        app.init_resource::<GifCaptureSettings>();
        app.add_system(read_capture_events_and_tick_timer);
        static GET_GIF_DATA: &str = "get_gif_data";
        static GIF_CAPTURE: &str = "gif_capture";
        let render_app = app.get_sub_app_mut(RenderApp).unwrap();
        render_app
            .init_resource::<GifCaptureFrames>()
            .init_resource::<GifCaptureState>()
            .add_system_to_stage(RenderStage::Extract, extract_settings)
            .add_system_to_stage(RenderStage::Extract, extract_gif_capture)
            .add_system_to_stage(RenderStage::Prepare, create_buffer)
            .add_stage_before(
                RenderStage::Cleanup,
                GET_GIF_DATA,
                SystemStage::single_threaded(),
            )
            .add_system_to_stage(GET_GIF_DATA, write_gif)
            .add_system_to_stage(GET_GIF_DATA, save_gif_on_state);

        let mut render_graph = render_app.world.get_resource_mut::<RenderGraph>().unwrap();
        render_graph.add_node(GIF_CAPTURE, DispatchGifCapture {});
        //render_graph.iter_nodes().for_each(|x| println!("{:?}", x));
        render_graph.iter_sub_graphs().for_each(|x| {
            println!("Graph name: {}", x.0);
            x.1.iter_nodes().for_each(|y| println!("{:?}", y.name));
        });
        render_graph
            .add_node_edge(bevy::core_pipeline::core_2d::graph::NAME, GIF_CAPTURE)
            .unwrap();
        render_graph
            .add_node_edge(bevy::core_pipeline::core_3d::graph::NAME, GIF_CAPTURE)
            .unwrap();
    }
}

struct GifBuffer(Buffer);

/// Gets the buffer size needed to capture an entire window, where each pixel is a u32 color.
/// Output: (unpadded_bytes_per_row, padded_bytes_per_row, total_buffer_size)
fn get_buffer_size(window: &Window) -> (u32, usize, usize) {
    let pixel_size = mem::size_of::<[u8; 4]>() as u32;
    let unpadded_bytes_per_row = pixel_size * (window.width() as u32);
    let padded_bytes_per_row =
        RenderDevice::align_copy_bytes_per_row(unpadded_bytes_per_row as usize);
    let buffer_size = padded_bytes_per_row * (window.height() as usize);
    (unpadded_bytes_per_row, padded_bytes_per_row, buffer_size)
}

/// Creates the buffer for saving the gif based on the Windows size.
fn create_buffer(mut commands: Commands, render_device: Res<RenderDevice>, windows: Res<Windows>) {
    let primary_window = windows.primary();
    let (buffer_size, _, _) = get_buffer_size(primary_window);
    let buffer_desc = BufferDescriptor {
        size: buffer_size as u64,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        label: Some("Gif Output Buffer"),
        mapped_at_creation: false,
    };
    let output_buffer = render_device.create_buffer(&buffer_desc);
    commands.insert_resource(GifBuffer(output_buffer));
}

/// Writes the gif from the buffer, back into our frames resource.
fn write_gif(
    mut frames: ResMut<GifCaptureFrames>,
    buffer: Res<GifBuffer>,
    render_device: Res<RenderDevice>,
    windows: Res<Windows>,
) {
    let primary_window = windows.get_primary().unwrap();
    let (unpadded_bytes_per_row, padded_bytes_per_row, _) = get_buffer_size(primary_window);
    let buffer_slice = buffer.0.slice(..);
    render_device.map_buffer(&buffer_slice, MapMode::Read);
    let padded_data = buffer_slice.get_mapped_range();
    let data = padded_data
        .chunks(padded_bytes_per_row as _)
        .map(|chunk| &chunk[..unpadded_bytes_per_row as _])
        .flatten()
        .map(|x| *x)
        .collect::<Vec<_>>();
    drop(padded_data);
    //output_buffer.unmap();
    frames.0.push(data);
}

/// Saves the gif, if we just got finished capturing. Otherwise does nothing.
fn save_gif_on_state(
    settings: Res<GifCaptureSettings>,
    state: ResMut<GifCaptureState>,
    frames: Res<GifCaptureFrames>,
    windows: Res<Windows>,
    mut commands: Commands,
) {
    match state.as_ref() {
        GifCaptureState::Off => {}
        GifCaptureState::CurrentlyCapturing => {}
        GifCaptureState::JustFinishedCapturing => {
            let primary_window = windows.get_primary().unwrap();
            save_gif(
                settings.as_ref(),
                &frames.0,
                primary_window.width() as u16,
                primary_window.height() as u16,
            )
            .unwrap();
            commands.insert_resource(GifCaptureState::Off);
        }
    }
}

/// Creates a file, encodes the data from the Frames resource into the GIF format, and writes that data into the file.
fn save_gif(
    settings: &GifCaptureSettings,
    frames: &Vec<Vec<u8>>,
    width: u16,
    height: u16,
) -> Result<(), std::io::Error> {
    use gif::{Encoder, Frame};
    let mut image = std::fs::File::create(settings.path).unwrap();
    let encoder = Encoder::new(&mut image, width, height, &[]);
    if let Ok(mut encoder) = encoder {
        encoder.set_repeat(Repeat::Infinite).unwrap();
        for frame in frames {
            // Copying because the encoder can change the alpha value on pixels to 0xFF.
            let mutable_frame: &mut [u8] = &mut frame.clone();
            encoder
                .write_frame(&Frame::from_rgba_speed(
                    width,
                    height,
                    mutable_frame,
                    settings.speed,
                ))
                .unwrap();
        }
    }
    Ok(())
}
