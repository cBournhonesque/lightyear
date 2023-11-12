// We want to receive smooth updates for the other players' actions
// But we receive their actions at a given timestep that might not match the physics timestep.

// Which means we can do one of two things:
// - apply client prediction for all players
// - apply client prediction for the controlled player, and snapshot interpolation for the other players
