//! Opt-in playable lab bridge for the recovered registered/contact owner.
use bevy::prelude::*;
use crate::{game_menu::Page,ivp_runtime_scene::Scene};
#[derive(Resource,Default)]struct State{scene:Option<Scene>,origin:Vec3,rotation:Quat,error:Option<String>,auto_phase:u8,trace_written:bool}
#[derive(Component)]struct Draw(usize);
#[derive(Component)]struct Panel;
pub(crate) fn install(app:&mut App){
 if !std::env::args().any(|a|a=="--lab-ivp"){return;}
 app.init_resource::<State>().add_systems(Update,drive);
}
fn trace_path()->Option<std::path::PathBuf>{let args=std::env::args().collect::<Vec<_>>();args.windows(2).find(|a|a[0]=="--lab-ivp-trace").map(|a|a[1].clone().into())}
fn drive(mut commands:Commands,time:Res<Time>,keys:Res<ButtonInput<KeyCode>>,frontend:Res<crate::game_menu::Frontend>,game:Res<crate::game_state::Match>,session:Res<crate::campaign_settings::SessionSettings>,console:Res<crate::game_console::Console>,player:Single<&crate::game::Body>,camera:Single<Entity,With<crate::game::ViewCamera>>,collision:Res<crate::collision::Collision>,mut state:ResMut<State>,mut meshes:ResMut<Assets<Mesh>>,mut materials:ResMut<Assets<StandardMaterial>>,mut draws:Query<(Entity,&Draw,&mut Transform)>,mut panels:Query<(Entity,&mut Text,&mut Visibility),With<Panel>>){
 let lab=crate::test_lab::active(&session.map);let playing=frontend.page==Page::Game;
 if !lab{for (entity,_,_) in &mut draws{commands.entity(entity).despawn();}for (entity,_,_) in &mut panels{commands.entity(entity).despawn();}state.scene=None;state.error=None;return;}
 if !playing{for (_,_,mut v) in &mut panels{*v=Visibility::Hidden;}return;}
 let reset=keys.just_pressed(KeyCode::KeyL)&&!console.open;
 if (state.scene.is_none()&&state.error.is_none()||draws.is_empty()&&state.scene.is_some()||reset)&&!game.paused{
  match Scene::new(){Ok(scene)=>{state.scene=Some(scene);state.error=None;state.auto_phase=0;state.trace_written=false;},Err(error)=>{state.error=Some(error.into());}}
  let forward=Quat::from_rotation_y(player.yaw)*Vec3::NEG_Z;let right=forward.cross(Vec3::Y);let eye=player.feet+Vec3::Y*crate::collision::EYE;let distance=(collision.sight_trace(eye,eye+forward*3.).fraction*3.-0.6).clamp(1.3,3.);
  state.origin=eye+forward*distance-right*0.8;state.rotation=Quat::from_mat3(&Mat3::from_cols(right,Vec3::Y,-forward));
  if draws.is_empty(){for id in 0..3{let mesh=if id<2{meshes.add(Sphere::new(16.1*0.0254))}else{meshes.add(Cuboid::new(12.*0.0254,5.*0.0254,10.*0.0254))};let color=match id{0=>Color::srgb(0.65,0.68,0.72),1=>Color::srgb(0.12,0.65,1.),_=>Color::srgb(1.,0.5,0.08)};commands.spawn((Draw(id),Mesh3d(mesh),MeshMaterial3d(materials.add(StandardMaterial{base_color:color,unlit:true,..default()})),Transform::default(),bevy::camera::visibility::NoFrustumCulling,bevy::light::NotShadowCaster));}}
  if panels.is_empty(){commands.spawn((Panel,UiTargetCamera(*camera),Text::new("Recovered IVP lab"),TextFont{font_size:FontSize::Px(15.),..default()},TextColor(Color::srgb(0.65,0.9,1.)),TextShadow::default(),Node{position_type:PositionType::Absolute,left:px(12.),top:px(245.),width:px(690.),..default()}));}
  println!("IVP playable lab: created source-owned spheres, polygon body, contact and joints");
 }
 let auto=std::env::args().any(|a|a=="--lab-ivp-autotest");
 if !game.paused&&!console.open&&state.error.is_none(){
  let step=state.scene.as_ref().map_or(0,|s|s.steps);let kick=keys.just_pressed(KeyCode::KeyJ)||auto&&(state.auto_phase==0&&step>=45||state.auto_phase==2&&step>=120);let release=keys.just_pressed(KeyCode::KeyK)||auto&&state.auto_phase==1&&step>=90;
  let result=if let Some(scene)=state.scene.as_mut(){if kick{scene.kick()}else if release{scene.release()}else{Ok(())}}else{Ok(())};
  if let Err(e)=result{state.error=Some(e.into());}else if kick||release{if auto{state.auto_phase+=1;}println!("IVP playable lab: {} at step {step}",if kick{"impulse"}else{"joint release"});}
  if state.error.is_none(){if let Some(scene)=state.scene.as_mut(){if let Err(e)=scene.frame(time.delta_secs_f64(),false){state.error=Some(e.into());}}}
 }
 if let Some(scene)=state.scene.as_ref(){let poses=scene.poses();for (_,draw,mut transform) in &mut draws{if let Some((p,q))=poses.get(draw.0){transform.translation=state.origin+state.rotation**p;transform.rotation=state.rotation**q;}}
  let label=format!("RECOVERED IVP LAB | J impulse | K release joints | L reset\n{} steps | {} contact systems | joints {}\nIsolated lab bodies: map collisions and campaign corpses are not connected.{}",scene.steps,scene.world.contacts().system_ids().len(),if scene.joints{"on"}else{"released"},state.error.as_ref().map_or(String::new(),|e|format!("\nStopped: {e}")));
  for (_,mut text,mut visibility) in &mut panels{**text=label.clone();*visibility=Visibility::Inherited;}
  if scene.steps>=150&&!state.trace_written{if let Some(path)=trace_path(){let report=serde_json::json!({"steps":scene.steps,"joints":scene.joints,"contact_systems":scene.world.contacts().system_ids(),"error":state.error,"bodies":scene.world.bodies().iter().map(|b|serde_json::json!({"id":b.id,"position":b.current.position.to_array(),"linear":b.linear,"angular":b.angular,"flags":b.flags})).collect::<Vec<_>>(),"poses":poses.iter().map(|(p,q)|serde_json::json!({"position":p.to_array(),"rotation":q.to_array()})).collect::<Vec<_>>()});match std::fs::write(&path,serde_json::to_string_pretty(&report).unwrap()){Ok(())=>println!("IVP playable lab trace: {}",path.display()),Err(e)=>eprintln!("IVP lab trace failed: {e}")}}
   state.trace_written=true;
  }
 }
}
