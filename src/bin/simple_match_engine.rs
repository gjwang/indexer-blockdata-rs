use std::collections::{BTreeMap, HashMap, VecDeque};


#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone)]
pub struct Order {
    pub id: u64,
    pub asset_id: u64,
    pub side: Side,
    pub price: u64,
    pub quantity: u64,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct Trade {
    pub match_id: u64,
    pub buy_order_id: u64,
    pub sell_order_id: u64,
    pub price: u64,
    pub quantity: u64,
}

pub struct OrderBook {
    asset_id: u64,
    // Bids: High to Low. We use Reverse for BTreeMap to iterate from highest price.
    bids: BTreeMap<std::cmp::Reverse<u64>, VecDeque<Order>>,
    // Asks: Low to High.
    asks: BTreeMap<u64, VecDeque<Order>>,
    trade_history: Vec<Trade>,
    order_counter: u64,
    match_sequence: u64,
}

impl OrderBook {
    pub fn new(asset_id: u64) -> Self {
        OrderBook {
            asset_id,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            trade_history: Vec::new(),
            order_counter: 0,
            match_sequence: 0,
        }
    }

    pub fn add_order(&mut self, asset_id: u64, side: Side, price: u64, quantity: u64) -> Result<u64, String> {
        if asset_id != self.asset_id {
            return Err(format!("Order asset_id {} does not match OrderBook asset_id {}", asset_id, self.asset_id));
        }

        self.order_counter += 1;
        let order = Order {
            id: self.order_counter,
            asset_id,
            side,
            price,
            quantity,
            timestamp: self.order_counter, // Using counter as logical timestamp
        };

        Ok(self.match_order(order))
    }

    fn match_order(&mut self, mut order: Order) -> u64 {
        let order_id = order.id;
        let mut trades = Vec::new();

        match order.side {
            Side::Buy => {
                // Match against Asks (Low to High)
                while order.quantity > 0 {
                    // Check if there is a matching ask
                    let mut best_ask_price = None;
                    if let Some((price, _)) = self.asks.iter().next() {
                        if price <= &order.price {
                            best_ask_price = Some(*price);
                        }
                    }

                    if let Some(price) = best_ask_price {
                        let orders_at_price = self.asks.get_mut(&price).unwrap();
                        while let Some(mut best_ask) = orders_at_price.pop_front() {
                            let trade_quantity = u64::min(order.quantity, best_ask.quantity);
                            
                            self.match_sequence += 1;
                            trades.push(Trade {
                                match_id: self.match_sequence,
                                buy_order_id: order.id,
                                sell_order_id: best_ask.id,
                                price: best_ask.price, // Trade happens at maker's price
                                quantity: trade_quantity,
                            });

                            order.quantity -= trade_quantity;
                            best_ask.quantity -= trade_quantity;

                            if best_ask.quantity > 0 {
                                // If maker order is not fully filled, push it back to front (it has priority)
                                orders_at_price.push_front(best_ask);
                                break; // Taker is fully filled
                            }
                            
                            if order.quantity == 0 {
                                break;
                            }
                        }
                        
                        // Clean up empty price level
                        if orders_at_price.is_empty() {
                            self.asks.remove(&price);
                        }
                    } else {
                        break; // No matching asks
                    }
                }
            }
            Side::Sell => {
                // Match against Bids (High to Low)
                while order.quantity > 0 {
                    let mut best_bid_price = None;
                    if let Some((std::cmp::Reverse(price), _)) = self.bids.iter().next() {
                        if price >= &order.price {
                            best_bid_price = Some(*price);
                        }
                    }

                    if let Some(price) = best_bid_price {
                        let orders_at_price = self.bids.get_mut(&std::cmp::Reverse(price)).unwrap();
                        while let Some(mut best_bid) = orders_at_price.pop_front() {
                            let trade_quantity = u64::min(order.quantity, best_bid.quantity);

                            self.match_sequence += 1;
                            trades.push(Trade {
                                match_id: self.match_sequence,
                                buy_order_id: best_bid.id,
                                sell_order_id: order.id,
                                price: best_bid.price,
                                quantity: trade_quantity,
                            });

                            order.quantity -= trade_quantity;
                            best_bid.quantity -= trade_quantity;

                            if best_bid.quantity > 0 {
                                orders_at_price.push_front(best_bid);
                                break;
                            }

                            if order.quantity == 0 {
                                break;
                            }
                        }

                        if orders_at_price.is_empty() {
                            self.bids.remove(&std::cmp::Reverse(price));
                        }
                    } else {
                        break;
                    }
                }
            }
        }

        // If order still has quantity, add to book
        if order.quantity > 0 {
            match order.side {
                Side::Buy => {
                    self.bids
                        .entry(std::cmp::Reverse(order.price))
                        .or_insert_with(VecDeque::new)
                        .push_back(order);
                }
                Side::Sell => {
                    self.asks
                        .entry(order.price)
                        .or_insert_with(VecDeque::new)
                        .push_back(order);
                }
            }
        }

        self.trade_history.extend(trades.clone());
        
        for trade in trades {
            println!("Trade Executed: {:?}", trade);
        }

        order_id
    }

    pub fn print_book(&self) {
        println!("ASKS:");
        for (price, orders) in self.asks.iter().rev() {
            let total_qty: u64 = orders.iter().map(|o| o.quantity).sum();
            println!("  Price: {} | Qty: {} | Orders: {}", price, total_qty, orders.len());
        }
        println!("------------------");
        println!("BIDS:");
        for (std::cmp::Reverse(price), orders) in self.bids.iter() {
            let total_qty: u64 = orders.iter().map(|o| o.quantity).sum();
            println!("  Price: {} | Qty: {} | Orders: {}", price, total_qty, orders.len());
        }
    }
}

pub struct MatchingEngine {
    order_books: Vec<OrderBook>,
    asset_map: HashMap<u64, String>,
}

impl MatchingEngine {
    pub fn new() -> Self {
        MatchingEngine {
            order_books: Vec::new(),
            asset_map: HashMap::new(),
        }
    }

    pub fn add_asset(&mut self, asset_id: u64, name: String) {
        if self.asset_map.contains_key(&asset_id) {
            panic!("Asset ID already exists");
        }
        self.asset_map.insert(asset_id, name);
        // Ensure vector is large enough
        if asset_id as usize >= self.order_books.len() {
            while self.order_books.len() <= asset_id as usize {
                let next_id = self.order_books.len() as u64;
                self.order_books.push(OrderBook::new(next_id));
            }
        }
    }

    pub fn add_order(&mut self, asset_id: u64, side: Side, price: u64, quantity: u64) -> Result<u64, String> {
        if !self.asset_map.contains_key(&asset_id) {
            return Err(format!("Asset ID {} not found", asset_id));
        }
        
        let book = &mut self.order_books[asset_id as usize];
        book.add_order(asset_id, side, price, quantity)
    }

    pub fn print_order_book(&self, asset_id: u64) {
        if let Some(name) = self.asset_map.get(&asset_id) {
            println!("\n--- Order Book for {} (ID: {}) ---", name, asset_id);
            self.order_books[asset_id as usize].print_book();
            println!("----------------------------------\n");
        } else {
            println!("Asset ID {} not found", asset_id);
        }
    }
}

fn main() {
    let mut engine = MatchingEngine::new();

    let btc_id = 1;
    let eth_id = 2;

    engine.add_asset(btc_id, "BTC_USDT".to_string());
    engine.add_asset(eth_id, "ETH_USDT".to_string());

    println!(">>> Trading BTC_USDT");
    println!("Adding Sell Order: 100 @ 10");
    engine.add_order(btc_id, Side::Sell, 100, 10).unwrap();
    
    println!("Adding Sell Order: 101 @ 5");
    engine.add_order(btc_id, Side::Sell, 101, 5).unwrap();

    engine.print_order_book(btc_id);

    println!("Adding Buy Order: 100 @ 8 (Should match partial 100)");
    engine.add_order(btc_id, Side::Buy, 100, 8).unwrap();

    engine.print_order_book(btc_id);

    println!(">>> Trading ETH_USDT");
    println!("Adding Sell Order: 2000 @ 50");
    engine.add_order(eth_id, Side::Sell, 2000, 50).unwrap();
    
    engine.print_order_book(eth_id);
    
    println!(">>> Trading BTC_USDT again");
    println!("Adding Buy Order: 102 @ 10 (Should match remaining 2 @ 100 and 5 @ 101, rest 3 on book)");
    engine.add_order(btc_id, Side::Buy, 102, 10).unwrap();

    engine.print_order_book(btc_id);
}
