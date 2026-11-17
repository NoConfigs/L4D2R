//! Numeric public ragdoll construction (10088e20/100bd600/100ba490),
//! including 10086e20's hinge selection. Existing body/object frames are inputs.
use bevy::prelude::Vec3;
use crate::{ivp_group_complete::Member,ivp_joint_mixed::Joint,ivp_joint_angular::Axis,ivp_motion::ObjectFrame};
use crate::ivp_hinge_constructor::{inverse,forward};
#[derive(Clone,Copy,Debug)]
pub(crate) struct Limit{pub min:f32,pub max:f32,pub velocity:f32,pub torque:f32}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Parameters{pub frames:[[[f32;4];3];2],pub axes:[Limit;3],pub force_limit:f32,pub torque_limit:f32,pub mass_scale:[f32;2],pub angular_only:bool,pub clockwise:bool}
#[cfg(any(target_arch="x86",target_arch="x86_64"))]
pub(crate) fn sin(x:f32)->f32{let mut out=0f32;unsafe{core::arch::asm!("fld dword ptr [{x}]","fsin","fstp dword ptr [{out}]",x=in(reg)&x,out=in(reg)&mut out,options(nostack,preserves_flags));}out}
#[cfg(any(target_arch="x86",target_arch="x86_64"))]
pub(crate) fn cos(x:f32)->f32{let mut out=0f32;unsafe{core::arch::asm!("fld dword ptr [{x}]","fcos","fstp dword ptr [{out}]",x=in(reg)&x,out=in(reg)&mut out,options(nostack,preserves_flags));}out}
#[cfg(any(target_arch="x86",target_arch="x86_64"))]
fn cone_cos(low:f32,high:f32)->f32{let half=0.5f32;let mut out=0f32;unsafe{core::arch::asm!("fld dword ptr [{hi}]","fsub dword ptr [{lo}]","fmul dword ptr [{half}]","fcos","fstp dword ptr [{out}]",hi=in(reg)&high,lo=in(reg)&low,half=in(reg)&half,out=in(reg)&mut out,options(nostack,preserves_flags));}out}
#[cfg(any(target_arch="x86",target_arch="x86_64"))]
fn torque_units(mass:f32,torque:f32)->f32{let unit=f32::from_bits(0x42652ee1);let mut out=0f32;unsafe{core::arch::asm!("fld dword ptr [{mass}]","fmul dword ptr [{torque}]","fmul dword ptr [{unit}]","fstp dword ptr [{out}]",mass=in(reg)&mass,torque=in(reg)&torque,unit=in(reg)&unit,out=in(reg)&mut out,options(nostack,preserves_flags));}out}
#[cfg(not(any(target_arch="x86",target_arch="x86_64")))]
pub(crate) fn sin(x:f32)->f32{x.sin()}
#[cfg(not(any(target_arch="x86",target_arch="x86_64")))]
pub(crate) fn cos(x:f32)->f32{x.cos()}
#[cfg(not(any(target_arch="x86",target_arch="x86_64")))]
fn cone_cos(low:f32,high:f32)->f32{(((high as f64-low as f64)*0.5).cos()) as f32}
#[cfg(not(any(target_arch="x86",target_arch="x86_64")))]
fn torque_units(mass:f32,torque:f32)->f32{(mass as f64*torque as f64*f32::from_bits(0x42652ee1) as f64) as f32}
fn local_frame(m:[[f32;4];3])->[[f32;3];3]{[[m[0][0],-m[2][0],m[1][0]],[-m[0][2],m[2][2],-m[1][2]],[m[0][1],-m[2][1],m[1][1]]]}
fn converted(v:[f32;3])->[f32;3]{[v[0],-v[2],v[1]]}
fn rotate(m:[[f32;3];3],axis:usize,angle:f32)->[[f32;3];3]{
    let (i,j)=((axis+1)%3,(axis+2)%3);let mut r=[[0.;3];3];r[axis][axis]=1.;r[i][i]=cos(angle);r[j][j]=cos(angle);r[i][j]=sin(angle);r[j][i]=-sin(angle);
    std::array::from_fn(|c|std::array::from_fn(|k|(r[c][0]*m[0][k]+r[c][1]*m[1][k])+r[c][2]*m[2][k]))
}
fn object_pose(m:Member)->crate::ivp_motion::Pose{let mut pose=m.body.current;if let Some(offset)=m.object_offset{pose.position=pose.transform_point(Vec3::from_array(offset));}pose}
fn angular_diagonal(v:[f32;3],inv:[f32;4],old:f32)->f32{(((0.+v[1]*(v[1]*inv[1]))+v[0]*(v[0]*inv[0]))+(v[2]*inv[2])*v[2])+old}
fn lever(v:[f32;3],arm:[f32;3])->f32{let x=v[2]*arm[1]-v[1]*arm[2];let y=v[0]*arm[2]-arm[0]*v[2];let z=arm[0]*v[1]-v[0]*arm[1];(y*y+x*x)+z*z}
fn choose(frames:[[[f32;3];3];2],anchors:[[f32;3];2],b:[Member;2])->usize{
    let arms=std::array::from_fn::<_,2,_>(|s|(object_pose(b[s]).transform_point(Vec3::from_array(anchors[s]))-b[s].body.current.position).to_array());
    let mut chosen=0;let mut best=-1.;
    for axis in 0..3{
        let world=std::array::from_fn::<_,2,_>(|s|forward(b[s].body.current.rotation,frames[s][axis]));
        let local0=inverse(b[0].body.current.rotation,world[0]);let local1=inverse(b[1].body.current.rotation,world[0].map(|x|-x));
        // Installed 100bd600 calls 100be650 on body0 twice, with each body's
        // angular row. Preserve that identity and its fixed/skip-force gate.
        let diagonal=if b[0].body.flags&0x12!=0{0.}else{angular_diagonal(local1,b[0].body.inverse,angular_diagonal(local0,b[0].body.inverse,0.))};
        let score=(lever(world[0],arms[0])*b[0].body.inverse[3]+diagonal)+lever(world[1],arms[1])*b[1].body.inverse[3];
        if best<score{best=score;chosen=axis;}
    }chosen
}
fn hinge(p:Parameters,b:[Member;2],axis:usize)->Result<Joint,&'static str>{
    let sample=b[0].body.prediction.sample(b[0].body.clock,ObjectFrame{shift:b[0].object_offset.map(Vec3::from_array),rotation:None});let r=sample.rotation;let scale=f32::from_bits(1109228278);
    let body=[[r.x_axis.x,r.z_axis.x,-r.y_axis.x],[r.x_axis.z,r.z_axis.z,-r.y_axis.z],[-r.x_axis.y,-r.z_axis.y,r.y_axis.y]];
    let position=[sample.position.x*scale,sample.position.z*scale,-(sample.position.y*scale)];let reference=p.frames[0];
    let world_position=std::array::from_fn(|i|((body[i][2]*reference[2][3]+body[i][1]*reference[1][3])+body[i][0]*reference[0][3])+position[i]);
    let world_axis=std::array::from_fn(|i|((body[i][2]*reference[2][axis]+body[i][1]*reference[1][axis])+body[i][0]*reference[0][axis])+0.);
    let mut perp=[0.;3];perp[(axis+1)%3]=1.;
    let local=std::array::from_fn::<_,3,_>(|i|(reference[1][i]*perp[1]+reference[0][i]*perp[0])+reference[2][i]*perp[2]);
    let attached=p.frames[1];let other=std::array::from_fn(|i|(attached[i][1]*local[1]+attached[i][0]*local[0])+attached[i][2]*local[2]);
    let a=p.axes[axis];let limits=if p.clockwise{[a.min,a.max]}else{[-a.max,-a.min]};
    let parameters=crate::ivp_hinge_constructor::Parameters{position:world_position,direction:world_axis,limits,velocity:a.velocity,torque:torque_units(b[0].body.mass,a.torque),strength:1.,force_limit:p.force_limit,torque_limit:p.torque_limit,mass_scale:p.mass_scale};
    Ok(Joint::Hinge(crate::ivp_hinge_constructor::construct_extended(parameters,b,[perp,other])?))
}
pub(crate) fn construct(p:Parameters,b:[Member;2])->Result<Joint,&'static str>{
    if !cfg!(any(target_arch="x86",target_arch="x86_64")){return Err("native ragdoll constructor arithmetic backend unavailable");}
    if p.frames.into_iter().flatten().flatten().chain(p.axes.into_iter().flat_map(|a|[a.min,a.max,a.velocity,a.torque])).chain([p.force_limit,p.torque_limit]).chain(p.mass_scale).any(|v|!v.is_finite()){return Err("non-finite public ragdoll parameters");}
    if [p.force_limit,p.torque_limit].into_iter().any(|v|v!=0.&&v<f32::MAX)||p.mass_scale.into_iter().any(|v|v!=0.&&v!=1.){return Err("breakable ragdoll wrapper is not owned");}
    let free=p.axes.iter().enumerate().filter(|(_,a)|a.min!=a.max).map(|(i,_)|i).collect::<Vec<_>>();if free.len()==1{return hinge(p,b,free[0]);}
    let radians=f32::from_bits(0x3c8efa35);let unit=f32::from_bits(0x3cd013a9);let mapping=[0,2,1];let mut low=[0.;3];let mut high=[0.;3];
    for src in 0..3{let a=p.axes[src];let values=if src==2{[-a.max*radians,-a.min*radians]}else{[a.min*radians,a.max*radians]};let values=if p.clockwise{[-values[1],-values[0]]}else{values};low[mapping[src]]=values[0];high[mapping[src]]=values[1];}
    let spans=std::array::from_fn::<_,3,_>(|i|high[i]-low[i]);let centers=std::array::from_fn::<_,3,_>(|i|(low[i]+high[i])*0.5);
    let mut frames=p.frames.map(local_frame);let anchors=p.frames.map(|m|converted([m[0][3]*unit,m[1][3]*unit,m[2][3]*unit]));
    let positive=(0..3).filter(|&i|spans[i]>f32::from_bits(0x24e69595)).collect::<Vec<_>>();
    let mut limits=[[0.,0.],[-0.1,0.1],[0.,0.]];let mut permutation=[0,1,2];
    if !positive.is_empty(){
        let (twist,plane,cone)=if positive.len()==1{let cone=positive[0];((cone+1)%3,(cone+2)%3,cone)}else{let twist=choose(frames,anchors,b);let(mut plane,mut cone)=((twist+1)%3,(twist+2)%3);if spans[cone]<spans[plane]{std::mem::swap(&mut cone,&mut plane);}(twist,plane,cone)};
        frames[0]=rotate(frames[0],cone,centers[cone]);frames=frames.map(|m|[m[twist],m[plane],m[cone]]);permutation=[twist,cone,plane];
        let span=if positive.len()==1&&spans[cone]>std::f32::consts::PI&&spans[cone]<std::f32::consts::TAU{std::f32::consts::PI}else{spans[cone]};
        limits[1]=[-span*0.5,span*0.5];if positive.len()>1{limits[0]=[-high[twist],-low[twist]];limits[2]=[low[plane],high[plane]];}
    }
    let mut axes=[Axis{tau:1.,..Default::default()};3];let mut enabled=[true;3];
    if !positive.is_empty(){for i in 0..3{enabled[i]=(limits[i][0]+std::f32::consts::PI).abs()>0.0001||(limits[i][1]-std::f32::consts::PI).abs()>0.0001;}}
    for i in 0..3{axes[i].min=limits[i][0];axes[i].max=limits[i][1];if axes[i].max-axes[i].min>=std::f32::consts::TAU{enabled[i]=false;}}
    for src in 0..3{let slot=permutation.iter().position(|&i|i==mapping[src]).unwrap();let a=p.axes[src];let force=b[0].body.mass as f64*a.torque as f64;axes[slot].force=(force as f32).abs();axes[slot].motor=force!=0.;axes[slot].velocity=a.velocity*radians;}
    axes[1].min=cone_cos(limits[1][0],limits[1][1]);axes[1].max=100.;axes[2].min=-sin(limits[2][1]);axes[2].max=-sin(limits[2][0]);
    Ok(Joint::Ragdoll(crate::ivp_joint_callback::Joint{frames,anchors,axes,limit_enabled:enabled,linear_enabled:!p.angular_only,damping:1.,tau:1.,work:None}))
}
