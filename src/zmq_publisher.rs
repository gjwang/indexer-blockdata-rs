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

        let settlement_pub = context.socket(PUSH)?;
        settlement_pub.set_sndhwm(1_000_000)?;
        settlement_pub.bind(&format!("tcp://*:{}", settlement_port))?;
        println!("   [ZMQ] Settlement PUB bound to port {}", settlement_port);

        let market_data_pub = context.socket(PUB)?;
        market_data_pub.set_sndhwm(1_000_000)?;
        market_data_pub.bind(&format!("tcp://*:{}", market_data_port))?;
        println!("   [ZMQ] Market Data PUB bound to port {}", market_data_port);

        Ok(Self { _context: context, settlement_pub, market_data_pub })
    }

    pub fn publish_settlement(&self, data: &[u8]) -> Result<(), zmq::Error> {
        // Topic: "settlement"
        // Multipart: [Topic, Data]
        self.settlement_pub.send("settlement", zmq::SNDMORE)?;
        self.settlement_pub.send(data, 0)
    }

    pub fn publish_market_data(&self, data: &[u8]) -> Result<(), zmq::Error> {
        // Topic: "market_data"
        // Multipart: [Topic, Data]
        self.market_data_pub.send("market_data", zmq::SNDMORE)?;
        self.market_data_pub.send(data, 0)
    }
}
