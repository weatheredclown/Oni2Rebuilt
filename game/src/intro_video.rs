use bevy::prelude::*;
use crate::menu::AppState;
use crate::mpeg2_video::{Mpeg2VideoFinishedMessage, PlayMpeg2Video, VideoSource};

pub struct IntroVideoPlugin;

#[derive(Resource)]
struct IntroVideoState {
    current_video: usize,
    videos: Vec<String>,
}

impl Plugin for IntroVideoPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<crate::mpeg2_video::Mpeg2VideoPlugin>() {
            app.add_plugins(crate::mpeg2_video::Mpeg2VideoPlugin);
        }
        app.add_systems(OnEnter(AppState::IntroVideo), start_intro)
            .add_systems(
                Update,
                handle_video_finish.run_if(in_state(AppState::IntroVideo)),
            );
    }
}

fn start_intro(mut commands: Commands, mut play_events: MessageWriter<PlayMpeg2Video>) {
    let videos = vec![
        "movies/angel.m2v".to_string(),
        "movies/rockstarlogo.m2v".to_string(),
    ];

    info!("Starting intro videos sequence");

    play_events.write(PlayMpeg2Video {
        source: VideoSource::Vfs(videos[0].clone()),
    });

    commands.insert_resource(IntroVideoState {
        current_video: 0,
        videos,
    });
}

fn handle_video_finish(
    mut finished_events: MessageReader<Mpeg2VideoFinishedMessage>,
    mut play_events: MessageWriter<PlayMpeg2Video>,
    mut state: ResMut<IntroVideoState>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    for _ in finished_events.read() {
        state.current_video += 1;
        if state.current_video < state.videos.len() {
            info!("Intro video finished, playing next");
            play_events.write(PlayMpeg2Video {
                source: VideoSource::Vfs(state.videos[state.current_video].clone()),
            });
        } else {
            info!("Intro videos finished, transitioning to Menu");
            next_state.set(AppState::Menu);
        }
    }
}
