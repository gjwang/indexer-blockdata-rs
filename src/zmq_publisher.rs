use zmq::{Context, Socket, PUB, PUSH};

pub struct ZmqPublisher {
    _context: Context, // Keep context alive
    settlement_pub: Socket,
    market_data_pub: Socket,
}

// SAFETY: ZMQ sockets are not thread-safe, but we are using this publisher
// in a single-threaded Disruptor consumer. We ensure exclusive access by design.
unsafe impl Send for ZmqPublisher {}
unsafe impl Sync for ZmqPublisher {}

impl ZmqPublisher {
    pub fn new(settlement_port: u16, market_data_port: u16) -> Result<Self, zmq::Error> {
        let context = Context::new();

        // PUSH socket for settlement - BLOCKS when HWM reached (back-pressure)
        let settlement_pub = context.socket(PUSH)?;
        // Set finite HWM - when this many messages are queued, send() blocks
        settlement_pub.set_sndhwm(100)?; // Small buffer - forces back-pressure quickly
        settlement_pub.set_linger(-1)?; // Wait forever to send buffered messages on close
        settlement_pub.set_sndtimeo(-1)?; // Block forever on send when HWM reached
        settlement_pub.connect(&format!("tcp://localhost:{}", settlement_port))?;
        println!("   [ZMQ] Settlement PUSH connected to port {} (HWM=100, blocking)", settlement_port);

        let market_data_pub = context.socket(PUB)?;
        market_data_pub.set_sndhwm(1_000_000)?;
        market_data_pub.bind(&format!("tcp://*:{}", market_data_port))?;
        println!("   [ZMQ] Market Data PUB bound to port {}", market_data_port);

        Ok(Self { _context: context, settlement_pub, market_data_pub })
    }

    pub fn publish_settlement(&self, data: &[u8]) -> Result<(), zmq::Error> {
        // PUSH socket - no topic needed, just send data
        self.settlement_pub.send(data, 0)
    }

    pub fn publish_market_data(&self, data: &[u8]) -> Result<(), zmq::Error> {
        // Topic: "market_data"
        // Multipart: [Topic, Data]
        self.market_data_pub.send("market_data", zmq::SNDMORE)?;
        self.market_data_pub.send(data, 0)
    }

    /// Publish EngineOutput bundle to settlement service
    /// This is the preferred method for the new atomic output flow
    pub fn publish_engine_output(&self, output: &crate::engine_output::EngineOutput) -> Result<(), String> {
        match serde_json::to_vec(output) {
            Ok(data) => {
                self.settlement_pub.send(&data, 0)
                    .map_err(|e| format!("ZMQ send failed: {}", e))
            }
            Err(e) => Err(format!("Serialization failed: {}", e))
        }
    }
}
