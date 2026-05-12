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

fn start_intro(
    mut commands: Commands,
    mut play_events: MessageWriter<PlayMpeg2Video>,
    mut assets: ResMut<Assets<bevy::audio::AudioSource>>,
) {
    let videos = vec![
        "movies/angel.m2v".to_string(),
        "movies/rockstarlogo.m2v".to_string(),
    ];

    info!("Starting intro videos sequence");

    play_events.write(PlayMpeg2Video {
        source: VideoSource::Vfs(videos[0].clone()),
    });

    play_imf_audio(&videos[0], &mut commands, &mut assets);

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
    mut commands: Commands,
    mut assets: ResMut<Assets<bevy::audio::AudioSource>>,
) {
    for _ in finished_events.read() {
        state.current_video += 1;
        if state.current_video < state.videos.len() {
            info!("Intro video finished, playing next");
            let next_video = &state.videos[state.current_video];
            play_events.write(PlayMpeg2Video {
                source: VideoSource::Vfs(next_video.clone()),
            });
            play_imf_audio(next_video, &mut commands, &mut assets);
        } else {
            info!("Intro videos finished, transitioning to Menu");
            next_state.set(AppState::Menu);
        }
    }
}

fn play_imf_audio(
    video_path: &str,
    commands: &mut Commands,
    assets: &mut ResMut<Assets<bevy::audio::AudioSource>>,
) {
    let audio_path = video_path.replace(".m2v", ".imf");
    if let Ok(data) = crate::vfs::read("", &audio_path) {
        if let Some(audio_source) = crate::imf_audio::create_audio_source(&data) {
            let handle = assets.add(audio_source);
            commands.spawn((
                bevy::audio::AudioPlayer(handle),
                bevy::audio::PlaybackSettings::DESPAWN,
            ));
        }
    }
}
