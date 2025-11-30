use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};


#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Price(u64);

impl Price {
    pub fn new(value: u64) -> Self {
        Price(value)
    }

    pub fn value(&self) -> u64 {
        self.0
    }
}

impl PartialOrd for Price {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.0.cmp(&other.0))
    }
}

impl Ord for Price {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

#[derive(Debug, Clone)]
pub struct Order {
    pub id: u64,
    pub side: Side,
    pub price: Price,
    pub quantity: u64,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct Trade {
    pub buy_order_id: u64,
    pub sell_order_id: u64,
    pub price: Price,
    pub quantity: u64,
}

pub struct OrderBook {
    // Bids: High to Low. We use Reverse for BTreeMap to iterate from highest price.
    bids: BTreeMap<std::cmp::Reverse<Price>, VecDeque<Order>>,
    // Asks: Low to High.
    asks: BTreeMap<Price, VecDeque<Order>>,
    trade_history: Vec<Trade>,
    order_counter: u64,
}

impl OrderBook {
    pub fn new() -> Self {
        OrderBook {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            trade_history: Vec::new(),
            order_counter: 0,
        }
    }

    pub fn add_order(&mut self, side: Side, price: u64, quantity: u64) -> u64 {
        self.order_counter += 1;
        let order = Order {
            id: self.order_counter,
            side,
            price: Price::new(price),
            quantity,
            timestamp: self.order_counter, // Using counter as logical timestamp
        };

        self.match_order(order)
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
                            best_ask_price = Some(price.clone());
                        }
                    }

                    if let Some(price) = best_ask_price {
                        let orders_at_price = self.asks.get_mut(&price).unwrap();
                        while let Some(mut best_ask) = orders_at_price.pop_front() {
                            let trade_quantity = u64::min(order.quantity, best_ask.quantity);
                            
                            trades.push(Trade {
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
                            best_bid_price = Some(price.clone());
                        }
                    }

                    if let Some(price) = best_bid_price {
                        let orders_at_price = self.bids.get_mut(&std::cmp::Reverse(price)).unwrap();
                        while let Some(mut best_bid) = orders_at_price.pop_front() {
                            let trade_quantity = u64::min(order.quantity, best_bid.quantity);

                            trades.push(Trade {
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
        println!("\n--- Order Book ---");
        println!("ASKS:");
        for (price, orders) in self.asks.iter().rev() {
            let total_qty: u64 = orders.iter().map(|o| o.quantity).sum();
            println!("  Price: {} | Qty: {} | Orders: {}", price.value(), total_qty, orders.len());
        }
        println!("------------------");
        println!("BIDS:");
        for (std::cmp::Reverse(price), orders) in self.bids.iter() {
            let total_qty: u64 = orders.iter().map(|o| o.quantity).sum();
            println!("  Price: {} | Qty: {} | Orders: {}", price.value(), total_qty, orders.len());
        }
        println!("------------------\n");
    }
}

fn main() {
    let mut engine = OrderBook::new();

    println!("Adding Sell Order: 100 @ 10");
    engine.add_order(Side::Sell, 100, 10);
    
    println!("Adding Sell Order: 101 @ 5");
    engine.add_order(Side::Sell, 101, 5);

    engine.print_book();

    println!("Adding Buy Order: 100 @ 8 (Should match partial 100)");
    engine.add_order(Side::Buy, 100, 8);

    engine.print_book();

    println!("Adding Buy Order: 102 @ 10 (Should match remaining 2 @ 100 and 5 @ 101, rest 3 on book)");
    engine.add_order(Side::Buy, 102, 10);

    engine.print_book();
}
