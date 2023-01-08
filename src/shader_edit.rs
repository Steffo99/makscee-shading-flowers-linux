use super::*;

use geng::prelude::*;
use std::{path::PathBuf, sync::mpsc::Receiver};

use notify::*;

#[derive(clap::Args)]
pub struct ShaderEdit {
    pub config_path: String,
}

#[derive(Deserialize, geng::Assets)]
#[asset(json)]
#[serde(deny_unknown_fields)]
struct ShaderEditConfig {
    path: PathBuf,
    parameters: ShaderParameters,
    vertices: usize,
    instances: usize,
    fov: f32,
}

impl ShaderEdit {
    pub fn run(self, geng: &Geng) -> Box<dyn geng::State> {
        Box::new(EditState::new(geng, self.config_path))
    }
}

type Time = R32;

struct EditState {
    geng: Geng,
    time: Time,
    config_path: PathBuf,
    config: ShaderEditConfig,
    shader: Option<(PathBuf, ugli::Program)>,
    receiver: Receiver<DebouncedEvent>,
    watcher: RecommendedWatcher,
}

impl EditState {
    pub fn new(geng: &Geng, config_path: String) -> Self {
        // Load config
        let config_path = static_path().join(&config_path);
        let mut config = futures::executor::block_on(<ShaderEditConfig as geng::LoadAsset>::load(
            geng,
            &config_path,
        ))
        .expect("Failed to load config");
        config.path = static_path().join(&config.path);

        let shader_library_list =
            match futures::executor::block_on(<String as geng::LoadAsset>::load(
                geng,
                &static_path().join("shader_library/_list.json"),
            )) {
                Ok(list) => list,
                Err(error) => {
                    error!("Failed to load shader library list");
                    String::new()
                }
            };
        let shader_library_list: Vec<String> = serde_json::from_str(&shader_library_list)
            .context("Failed to parse shader library list")
            .unwrap();

        let shader_library_list: Vec<PathBuf> = shader_library_list
            .iter()
            .map(|path| static_path().join(path))
            .collect();

        debug!("path: {}", config.path.to_str().unwrap());
        // Load shader
        let program = futures::executor::block_on(<ugli::Program as geng::LoadAsset>::load(
            geng,
            &config.path,
        ))
        .expect("Failed to load shader");

        // Setup watcher
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher: RecommendedWatcher =
            notify::Watcher::new(tx, std::time::Duration::from_secs(1))
                .expect("Failed to initialize a watcher");
        watcher
            .watch(&config_path, notify::RecursiveMode::NonRecursive)
            .expect(&format!("Failed to start watching {config_path:?}"));
        watcher
            .watch(&config.path, notify::RecursiveMode::NonRecursive)
            .expect(&format!("Failed to start watching {:?}", config.path));
        shader_library_list.iter().for_each(|path| {
            watcher
                .watch(&path, notify::RecursiveMode::NonRecursive)
                .expect(&format!("Failed to start watching {:?}", path))
        });
        debug!("path {}", config_path.to_str().unwrap());

        Self {
            geng: geng.clone(),
            time: Time::ZERO,
            shader: Some((config.path.clone(), program)),
            config_path,
            config,
            watcher,
            receiver: rx,
        }
    }

    fn handle_notify(&mut self, event: notify::DebouncedEvent) {
        debug!("Notify event: {event:?}");
        match event {
            DebouncedEvent::NoticeWrite(path)
            | DebouncedEvent::Create(path)
            | DebouncedEvent::Write(path) => self.reload_path(path),
            DebouncedEvent::NoticeRemove(path) => {
                // (Neo)vim writes the file by removing and recreating it,
                // hence this hack
                self.switch_watch(&path, &path);
                self.reload_path(path);
            }
            DebouncedEvent::Remove(_) => todo!(),
            DebouncedEvent::Error(error, path) => {
                error!("Notify error on path {path:?}: {error}");
            }
            _ => {}
        }
    }

    fn reload_path(&mut self, path: PathBuf) {
        if path == self.config_path {
            self.config = ShaderEditConfig::load(&self.geng, path).expect("Failed to load config");
            self.reload_shader();
        } else {
            self.reload_shader();
        }
    }

    fn switch_watch(
        &mut self,
        old_path: impl AsRef<std::path::Path>,
        new_path: impl AsRef<std::path::Path>,
    ) {
        if let Err(error) = self.watcher.unwatch(old_path.as_ref()) {
            error!(
                "Failed to unwatch old shader path ({:?}): {error}",
                old_path.as_ref()
            );
        }
        if let Err(error) = self
            .watcher
            .watch(new_path.as_ref(), notify::RecursiveMode::NonRecursive)
        {
            error!(
                "Failed to start watching shader on {:?}: {error}",
                new_path.as_ref()
            );
        }
    }

    fn reload_shader(&mut self) {
        // Stop watching old shader if the path has changed
        if let Some(path) = self.shader.as_ref().map(|(path, _)| path.clone()) {
            debug!(
                "path {} config {}",
                path.to_str().unwrap(),
                self.config.path.to_str().unwrap()
            );
            if path != self.config.path {
                self.switch_watch(path, self.config.path.clone());
                debug!("switch");
            }
        }

        debug!("Load:=> {}", self.config.path.to_str().unwrap());
        // Reload shader
        let program = match futures::executor::block_on(<ugli::Program as geng::LoadAsset>::load(
            &self.geng,
            &self.config.path,
        )) {
            Ok(program) => program,
            Err(error) => {
                error!("Failed to load program: {error}");
                return;
            }
        };
        self.shader = Some((self.config.path.clone(), program));
        let shader_library_list =
            match futures::executor::block_on(<String as geng::LoadAsset>::load(
                &self.geng,
                &static_path().join("shader_library/_list.json"),
            )) {
                Ok(list) => list,
                Err(error) => {
                    error!("Failed to load shader library list");
                    return;
                }
            };
        let shader_library_list: Vec<String> = serde_json::from_str(&shader_library_list)
            .context("Failed to parse shader library list")
            .unwrap();

        for path in shader_library_list {
            let asset_path = static_path().join("shader_library").join(&path);
            let source = match futures::executor::block_on(<String as geng::LoadAsset>::load(
                &self.geng,
                &asset_path,
            )) {
                Ok(source) => source,
                Err(error) => {
                    format!("Failed to load {:?}", asset_path);
                    return;
                }
            };
            self.geng.shader_lib().add(path.as_str(), &source);
        }
    }
}

impl ShaderEditConfig {
    fn load(geng: &Geng, full_path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let mut config = futures::executor::block_on(<ShaderEditConfig as geng::LoadAsset>::load(
            geng,
            full_path.as_ref(),
        ))?;
        config.path = static_path().join(&config.path);
        Ok(config)
    }
}

impl geng::State for EditState {
    fn update(&mut self, delta_time: f64) {
        let delta_time = Time::new(delta_time as _);
        self.time += delta_time;

        use std::sync::mpsc::TryRecvError;
        match self.receiver.try_recv() {
            Ok(event) => self.handle_notify(event),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                error!("Disconnected from the channel");
            }
        }
    }

    fn draw(&mut self, framebuffer: &mut ugli::Framebuffer) {
        ugli::clear(framebuffer, Some(Rgba::BLACK), None, None);

        let camera = geng::Camera2d {
            center: vec2(0.0, 0.0),
            rotation: 0.0,
            fov: self.config.fov,
        };

        if let Some((_, program)) = &self.shader {
            let vert_count = self.config.vertices;
            let mut vertices = vec![draw_2d::Vertex {
                a_pos: vec2(-1.0, -1.0),
            }];
            for i in 0..vert_count {
                vertices.push(draw_2d::Vertex {
                    a_pos: vec2((i as f32 / vert_count as f32) * 2.0 - 1.0, 1.0),
                });
                vertices.push(draw_2d::Vertex {
                    a_pos: vec2(((i + 1) as f32 / vert_count as f32) * 2.0 - 1.0, -1.0),
                });
            }

            vertices.push(draw_2d::Vertex {
                a_pos: vec2(1.0, 1.0),
            });

            let quad = ugli::VertexBuffer::new_static(self.geng.ugli(), vertices);
            let uniforms = (
                ugli::uniforms! {
                    u_time: self.time.as_f32(),
                    u_unit_position: Vec2::<f32>::ZERO,
                    u_unit_radius: 1_f32,
                    u_window_size: self.geng.window().size(),
                    u_spawn: 1.0 - self.time.as_f32().fract(),
                },
                geng::camera2d_uniforms(&camera, framebuffer.size().map(|x| x as f32)),
                &self.config.parameters,
            );
            let mut instances: ugli::VertexBuffer<Instance> =
                ugli::VertexBuffer::new_dynamic(self.geng.ugli(), Vec::new());
            instances.resize(self.config.instances, Instance {});
            ugli::draw(
                framebuffer,
                program,
                ugli::DrawMode::TriangleStrip,
                ugli::instanced(&quad, &instances),
                &uniforms,
                ugli::DrawParameters {
                    blend_mode: Some(ugli::BlendMode::default()),
                    ..default()
                },
            );
        }
    }
}
