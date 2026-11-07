//! Loads and renders a glTF file as a scene.

use bevy::{
    light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap},
    prelude::*,
    input::mouse::AccumulatedMouseMotion,
};

use std::f32::consts::*;


fn main() {
    App::new()
        // フラグリソースの初期化（このフレームで固定タイムステップが実行されたかを追跡）
        .init_resource::<DidFixedTimestepRunThisFrame>()
        .insert_resource(DirectionalLightShadowMap { size: 4096 })
        .add_plugins(DefaultPlugins)
        .add_systems(Startup, (setup, spawn_text, spawn_player))
        // 毎フレームの開始時（PreUpdate）：固定タイムステップ実行フラグをクリア
        .add_systems(PreUpdate, clear_fixed_timestep_flag)
        // 固定タイムステップの開始時（FixedPreUpdate）：実行フラグを立てる
        .add_systems(FixedPreUpdate, set_fixed_time_step_flag)
        // 固定タイムステップ（FixedUpdate）：一定時間刻みで物理位置を計算
        .add_systems(FixedUpdate, advance_physics)
        // RunFixedMainLoopスケジュール：固定タイムステップループの前後に実行するシステム群を配置
        .add_systems(
            RunFixedMainLoop,
            (
                // --- 固定タイムステップ計算の前に実行する処理 ---
                (
                    // カメラの回転：物理計算で「カメラの向きに応じた移動」を行うため、固定更新の前に回転を確定させる
                    rotate_camera,
                    // キーボード入力の蓄積：WASD入力を取得し、現在のカメラ向きに合わせて物理速度を設定
                    accumulate_input,
                )
                    .chain() // 順序依存（rotate_camera -> accumulate_input）を保証
                    .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),

                // --- 固定タイムステップ計算の後に実行する処理 ---
                (
                    // 固定タイムステップが1回以上実行された場合のみ、入力バッファをクリア
                    clear_input.run_if(did_fixed_timestep_run_this_frame),
                    // 物理位置（前回と今回）を線形補間し、見た目の描画位置（Transform）をスムーズに更新
                    interpolate_rendered_transform,
                    // カメラ位置をプレイヤーの補間後の表示位置に追従させる
                    translate_camera,
                )
                    .chain() // 順序依存を保証
                    .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
            ),
        )
        .add_systems(Update, animate_light_direction)
        .run();
}

/// プレイヤーのキーボード入力を蓄積するコンポーネント。
/// 前回物理シミュレーションが進んでから発生した全フレームの入力結果を保持します。
#[derive(Debug, Component, Clone, Copy, PartialEq, Default, Deref, DerefMut)]
struct AccumulatedInput {
    /// WASD入力から得られる2D移動方向ベクトル (x: 左右, y: 前後)
    movement: Vec2,
}

/// 物理シミュレーションにおけるプレイヤーの移動速度（単位: m/sメートル毎秒）。
/// Deref, DerefMut パターンにより `velocity.0` だけでなく直接 Vec3 として扱いやすくしています。
#[derive(Debug, Component, Clone, Copy, PartialEq, Default, Deref, DerefMut)]
struct Velocity(Vec3);

/// 物理シミュレーション内部でのプレイヤーの「現在の真の位置」。
/// 画面描画用の `Transform`（補間された位置）とは明確に分離して管理されます。
#[derive(Debug, Component, Clone, Copy, PartialEq, Default, Deref, DerefMut)]
struct PhysicalTranslation(Vec3);

/// 直前の固定タイムステップにおける物理位置。
/// フレーム描画時に `PhysicalTranslation` との間で線形補間（lerp）を行うために使用します。
#[derive(Debug, Component, Clone, Copy, PartialEq, Default, Deref, DerefMut)]
struct PreviousPhysicalTranslation(Vec3);

/// カメラのマウス操作感度を保持するコンポーネント。
/// 横方向（yaw）と縦方向（pitch）の感度を別々に設定できます。
#[derive(Debug, Component, Deref, DerefMut)]
struct CameraSensitivity(Vec2);

impl Default for CameraSensitivity {
    fn default() -> Self {
        Self(
            // 実用上、縦方向（Y）よりも横方向（X）の感度をやや高く設定すると操作感が良くなります。
            // 実行時にユーザー設定やインスペクターから調整できるようにコンポーネント化されています。
            Vec2::new(0.003, 0.002),
        )
    }
}

/// プレイヤーおよび3Dカメラをスポーン（生成）するシステム。
/// ※カメラをプレイヤーの小Entity（Child）にせず独立して生成することで、
/// プレイヤーモデルの回転がカメラの回転に不要な悪影響を与えないように設計されています。
fn spawn_player(mut commands: Commands) {
    // 3DカメラEntityの生成（カメラコンポーネントと操作感度コンポーネント）
    commands.spawn((Camera3d::default(), CameraSensitivity::default()));

    // プレイヤーEntityの生成（見えない物理カプセル/オブジェクトとして定義）
    commands.spawn((
        Name::new("Player"),
        // プレイヤーのサイズ調整（スケール 0.3倍）
        Transform::from_scale(Vec3::splat(0.3)),
        // 物理計算および移動に必要な各種コンポーネントをアタッチ
        AccumulatedInput::default(),
        Velocity::default(),
        PhysicalTranslation::default(),
        PreviousPhysicalTranslation::default(),
    ));
}

/* exclude fn spawn_environment */

/// 操作方法を画面左下に表示する2D UIテキストを生成するシステム。
fn spawn_text(mut commands: Commands) {
    let font = TextFont {
        font_size: FontSize::Px(25.0),
        ..default()
    };
    commands.spawn((
        // Flexboxレイアウトノード（画面左下に絶対配置）
        Node {
            position_type: PositionType::Absolute,
            bottom: px(12),
            left: px(12),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        // 子要素としてテキスト行を追加
        children![
            (Text::new("Move the player with WASD"), font.clone()),
            (Text::new("Rotate the camera with the mouse"), font)
        ],
    ));
}

/// マウスの移動量（AccumulatedMouseMotion）を取得し、カメラの回転（Transform.rotation）を更新するシステム。
fn rotate_camera(
    // 1フレーム中のマウス移動量の累積値リソース（Bevy 0.15+）
    accumulated_mouse_motion: Res<AccumulatedMouseMotion>,
    // Cameraコンポーネントを持つ唯一のEntityから Transform と CameraSensitivity を取得
    // Single<T> はワールド内に該当Entityがちょうど1個存在することを前提とする便利なクエリ型
    player: Single<(&mut Transform, &CameraSensitivity), With<Camera>>,
) {
    let (mut transform, camera_sensitivity) = player.into_inner();
    let delta = accumulated_mouse_motion.delta;

    if delta != Vec2::ZERO {
        // ※注意: マウス移動量には delta_time（時間経過）を掛け算しません。
        // マウス移動量は「前フレームからのピクセル移動距離そのもの」であるため、時間積算すると不自然に遅くなってしまいます。
        let delta_yaw = -delta.x * camera_sensitivity.x;   // 横回転（ヨー）
        let delta_pitch = -delta.y * camera_sensitivity.y; // 縦回転（ピッチ）

        // 現在のクォータニオン回転をオイラー角（Yaw, Pitch, Roll）に分解
        let (yaw, pitch, roll) = transform.rotation.to_euler(EulerRot::YXZ);
        let yaw = yaw + delta_yaw;

        // 【ジンバルロック・裏返り対策】
        // ピッチ（縦回転）が真上（+90度 = +π/2）または真下（-90度 = -π/2）に達すると、
        // 画面の上下反転やカメラの向きの不連続性（首が破綻する問題）が発生します。
        // これを防ぐため、真上・真下の少し手前（FRAC_PI_2 - 0.01）でピッチ角をクランプ（制限）します。
        const PITCH_LIMIT: f32 = FRAC_PI_2 - 0.01;
        let pitch = (pitch + delta_pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);

        // クランプされた新しいオイラー角からクォータニオンを再構築して代入
        transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll);
    }
    // println!("mouse = {:?}", accumulated_mouse_motion.delta);
}

/// キーボードのWASD入力を読み取り、カメラの向いている方向に基づいた移動速度（Velocity）に変換して蓄積するシステム。
fn accumulate_input(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    // プレイヤーの入力を保持する唯一のEntityを参照
    player: Single<(&mut AccumulatedInput, &mut Velocity)>,
    // カメラの向き（回転角）を参照
    camera: Single<&Transform, With<Camera>>,
) {
    // 歩行速度（Bevyの3D空間の基本単位は「メートル」なので 4.0 m/s = 秒速4m）
    const SPEED: f32 = 4.0;
    let (mut input, mut velocity) = player.into_inner();

    // 毎フレーム入力を一度リセット（押し続け入力に対応）
    input.movement = Vec2::ZERO;
    if keyboard_input.pressed(KeyCode::KeyW) {
        input.movement.y += 1.0;
    }
    if keyboard_input.pressed(KeyCode::KeyS) {
        input.movement.y -= 1.0;
    }
    if keyboard_input.pressed(KeyCode::KeyA) {
        input.movement.x -= 1.0;
    }
    if keyboard_input.pressed(KeyCode::KeyD) {
        input.movement.x += 1.0;
    }

    // 2D入力（WASD）をBevyの3D座標系（右手系：前方向は -Z 軸、上方向は +Y 軸）に変換
    // Wキー（+Y方向入力）を押した時、前に進むため Z 軸方向へは -input.movement.y と反転させます
    let input_3d = Vec3 {
        x: input.movement.x,
        y: 0.0,
        z: -input.movement.y,
    };

    // カメラの回転クォータニオンを入力ベクトルに乗算し、
    // 「カメラが見ている向きにおける前左右ベクトル」へ回転変換
    let rotated_input = camera.rotation * input_3d;

    // 斜め移動時に速度が √2 倍（約1.414倍）にならないよう正規化し、移動速度 SPEED を掛け算します。
    // ※ .normalize_or_zero() ではなく clamp_length_max(1.0) を使用する理由は、
    // ゲームパッドのアナログスティックで浅く倒した際（長さを1.0未満に保ちたい時）に対応するためです。
    velocity.0 = rotated_input.clamp_length_max(1.0) * SPEED;
}

/// このフレームで固定タイムステップが実行されたかどうかを示すフラグリソース。
#[derive(Resource, Debug, Deref, DerefMut, Default)]
pub struct DidFixedTimestepRunThisFrame(bool);

/// 毎フレームの開始時（PreUpdate）にフラグを false にリセットするシステム。
fn clear_fixed_timestep_flag(
    mut did_fixed_timestep_run_this_frame: ResMut<DidFixedTimestepRunThisFrame>,
) {
    did_fixed_timestep_run_this_frame.0 = false;
}

/// 固定タイムステップ実行時（FixedPreUpdate）にフラグを true に設定するシステム。
fn set_fixed_time_step_flag(
    mut did_fixed_timestep_run_this_frame: ResMut<DidFixedTimestepRunThisFrame>,
) {
    did_fixed_timestep_run_this_frame.0 = true;
}

/// 条件付きシステム実行（run_if）のための判定関数。
/// 固定タイムステップがこのフレームで実行された場合のみ true を返します。
fn did_fixed_timestep_run_this_frame(
    did_fixed_timestep_run_this_frame: Res<DidFixedTimestepRunThisFrame>,
) -> bool {
    did_fixed_timestep_run_this_frame.0
}

/// 固定タイムステップでの物理計算が完了した後、蓄積された入力をクリアするシステム。
fn clear_input(mut input: Single<&mut AccumulatedInput>) {
   **input = AccumulatedInput::default();
}

/// 固定タイムステップ（FixedUpdate）ごとに物理シミュレーションを1ステップ進めるシステム。
/// （※このシステムは1画面フレームの間に 0回、1回、または複数回 呼び出される可能性があります）
fn advance_physics(
    // FixedUpdate 内では Res<Time> は自動的に Res<Time<Fixed>> と同等になります（明示的に型指定しています）
    fixed_time: Res<Time<Fixed>>,
    mut query: Query<(
        &mut PhysicalTranslation,
        &mut PreviousPhysicalTranslation,
        &Velocity,
    )>,
) {
    for (mut current_physical_translation, mut previous_physical_translation, velocity) in
        query.iter_mut()
    {
        // 1. 今回の位置を「前回の物理位置」として退避
        previous_physical_translation.0 = current_physical_translation.0;
        // 2. 速度 × 固定タイムステップ時間（dt）を加えて「現在の物理位置」を更新
        current_physical_translation.0 += velocity.0 * fixed_time.delta_secs();
    }
}

/// 物理計算結果（前回と今回）の間を線形補間（lerp）し、
/// 見た目の描画用位置（Transform.translation）を滑らかに更新するシステム。
fn interpolate_rendered_transform(
    fixed_time: Res<Time<Fixed>>,
    mut query: Query<(
        &mut Transform,
        &PhysicalTranslation,
        &PreviousPhysicalTranslation,
    )>,
) {
    for (mut transform, current_physical_translation, previous_physical_translation) in
        query.iter_mut()
    {
        let previous = previous_physical_translation.0;
        let current = current_physical_translation.0;

        // fixed_time.overstep_fraction() は「次の固定ステップまでの余り時間の割合（0.0 〜 1.0）」を返します。
        // 例: 物理更新と次の物理更新のちょうど中間のタイミングで描画される場合、alpha は 0.5 になります。
        let alpha = fixed_time.overstep_fraction();

        // 前回位置と今回位置の間を alpha の割合で線形補間（lerp: Linear Interpolation）
        let rendered_translation = previous.lerp(current, alpha);

        // 計算したスムーズな位置をレンダリング用の Transform に反映
        transform.translation = rendered_translation;
    }
}

/// カメラの位置を、プレイヤーの補間後の表示位置（Transform.translation）に追従させるシステム。
fn translate_camera(
    // カメラの Transform
    mut camera: Single<&mut Transform, With<Camera>>,
    // プレイヤーの Transform（カメラ以外の AccumulatedInput を持つEntity）
    player: Single<&Transform, (With<AccumulatedInput>, Without<Camera>)>,
) {
    // カメラの位置をプレイヤーの位置と同一に更新（一人称視点）
    camera.translation = player.translation;
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn((
        //* Camera3d::default(),
        //* Transform::from_xyz(0.7, 0.7, 1.0).looking_at(Vec3::new(0.0, 0.3, 0.0), Vec3::Y),
        EnvironmentMapLight {
            diffuse_map: asset_server.load("environment_maps/pisa_diffuse_rgb9e5_zstd.ktx2"),
            specular_map: asset_server.load("environment_maps/pisa_specular_rgb9e5_zstd.ktx2"),
            intensity: 250.0,
            ..default()
        },
    ));

    commands.spawn((
        DirectionalLight {
            shadow_maps_enabled: true,
            ..default()
        },
        // This is a relatively small scene, so use tighter shadow
        // cascade bounds than the default for better quality.
        // We also adjusted the shadow map to be larger since we're
        // only using a single cascade.
        CascadeShadowConfigBuilder {
            num_cascades: 1,
            maximum_distance: 1.6,
            ..default()
        }
        .build(),
    ));
    commands.spawn(WorldAssetRoot(asset_server.load(
        GltfAssetLabel::Scene(0).from_asset("models/Exterior+home+design.glb"),
    )));
}

fn animate_light_direction(
    time: Res<Time>,
    mut query: Query<&mut Transform, With<DirectionalLight>>,
) {
    for mut transform in &mut query {
        transform.rotation = Quat::from_euler(
            EulerRot::ZYX,
            0.0,
            time.elapsed_secs() * PI / 5.0,
            -FRAC_PI_4,
        );
    }
}
