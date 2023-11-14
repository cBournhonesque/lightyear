use crate::inputs::input_buffer::InputMessage;
use crate::plugin::events::MessageEvent;
use crate::server::plugin::sets::ServerSet;
use crate::server::Server;
use crate::ClientId;
use crate::{App, Protocol};
use bevy::prelude::{EventReader, IntoSystemConfigs, Plugin, PreUpdate, ResMut};

// - ClientInputs:
// - inputs will be sent via a special message
// - in each packet, we will send the inputs for the last 10-15 frames. Can use the ring buffer?
// - First: send all inputs for last 15 frames. Along with the tick at which the input was sent
// - Maybe opt: Only send the inputs that have changed and the tick at which they change?
// - in the client: we don't send a packet every tick. So what we do is:
// - during fixedupdate, we store the input for the given tick. Store that input in a ringbuffer containing the input history. (for at least the rollback period)
// - at the end of the frame, we collect the last 15 ticks of inputs and put them in a packet.
// - we send that packet via tick-buffered sender, associated with the last client tick
// - IS THIS CORRECT APPROACH? IT WOULD MEAN THAT WE WOULD READ THAT PACKET ONLY ON THE CURRENT TICK IN THE SERVER, BUT ACTUALLY WE WANT TO READ IT IMMEDIATELY
// (BECAUSE IT CONTAINS LAST 15 TICKS OF INPUTS, SO CAN HELP FILL GAPS IN INPUTS!)
// - IT WOULD SEEM THAT WE CAN JUST SEND THE PACKET AS SEQUENCED-UNRELIABLE. (WE DONT NEED TO KNOW THE PACKET TICK BECAUSE IT CONTAINS TICKS)
// ON THE SERVER WE READ IMMEDIATELY AND WE UPDATE OUR RINGBUFFER OF INPUTS THAT WE CAN FETCH FROM!
// - during rollback, we can read from the input history
// - the input history is associated with a connection.
// - in the server, we receive the inputs, open the packet, and update the entire ringbuffer of inputs?
// - server is at tick 9. for example we didn't receive the input for tick 10,11; but we receive the packet for tick 12, which contains all the inputs for ticks 10,11,12.
pub struct InputPlugin<P: Protocol> {
    _marker: std::marker::PhantomData<P>,
}

impl<P: Protocol> Plugin for InputPlugin<P> {
    fn build(&self, app: &mut App) {
        // insert the input buffer resource
        app.add_systems(
            PreUpdate,
            update_input_buffer::<P>.after(ServerSet::Receive),
        );

        //right after receive, update the input buffer for each connection
        // FIXED UPDATE SYSTEM THAT CONSUMES INPUT FROM BUFFER! -> LET USER WRITE THAT
        // how does the user consume from buffer? provide a function in Server that returns the (inputs of all clients for the given tick)?
    }
}

// TODO: do it directly when receiving the message, not in a system
/// After receiving messages, we update the input buffer for each connection by reading the InputMessage
pub fn update_input_buffer<P: Protocol>(
    mut server: ResMut<Server<P>>,
    mut input_messages: EventReader<MessageEvent<InputMessage<P::Input>, ClientId>>,
) {
    for input_message in input_messages.read() {
        let client_id = input_message.context();
        let input_message = input_message.message();
        server.update_inputs(input_message, client_id);
    }
}

// on the client:
// - FixedUpdate: before physics but after increment tick,
//   - rollback: we get the input from the history -> HERE GIVE THE USER AN OPPORTUNITY TO CUSTOMIZE.
//        BY DEFAULT WE JUST TAKE THE INPUT FOR THE TICK, BUT MAYBE WE WANT TO DO SOMETHING ELSE?
//        SLIGHTLY MODIFY THE INPUT? IF NONE, REPEAT THE PREVIOUS ONE?
//   - non rollback:
//         we get the input from keyboard/mouse and store it in the InputBuffer
//         use input for predicted entities
//   - can use system piping?
// - Send:
//   - we read the
