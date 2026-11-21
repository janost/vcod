pub mod client_bullet;
pub mod cod2;
pub mod cod2_cloud;
pub mod cod2_tracer;
pub mod cod4_trail;
pub mod efx;
pub mod flame;
pub mod looped;
pub mod registry;
pub mod sim;
pub mod vehicle_trails;

use glam::Vec3;

/// `vectoangles`: pitch (down positive) and yaw of a direction, roll 0.
pub fn vectoangles(v: Vec3) -> Vec3 {
    if v.x == 0.0 && v.y == 0.0 {
        return Vec3::new(if v.z > 0.0 { -90.0 } else { 90.0 }, 0.0, 0.0);
    }
    let yaw = v.y.atan2(v.x).to_degrees();
    let pitch = -v.z.atan2(v.truncate().length()).to_degrees();
    Vec3::new(pitch, yaw, 0.0)
}
