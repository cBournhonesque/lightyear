// TODO: should we run the main loop at a fixed timestep? or just send/recv all events that were created during the frame?
//  is this just an optimization for the bandwidth, so we don't send packets as frequently; or packets that are less full?

// /// Runs the [`FixedUpdate`] schedule in a loop according until all relevant elapsed time has been "consumed".
// /// This is run by the [`Main`] schedule.
// #[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
// pub struct RunFixedUpdateLoop;
//
// /// The schedule that contains systems which only run after a fixed period of time has elapsed.
// ///
// /// The exclusive `run_fixed_update_schedule` system runs this schedule.
// /// This is run by the [`RunFixedUpdateLoop`] schedule.
// #[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
// pub struct FixedUpdate;
