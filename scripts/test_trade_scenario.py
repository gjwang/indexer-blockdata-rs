import subprocess
import time
import json
import urllib.request
import urllib.error
import sys
import os

# Configuration
GATEWAY_URL = "http://127.0.0.1:3001"
KEYSPACE = "trading"

# Paths: Determine absolute paths to project resources
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
PROJECT_ROOT = os.path.dirname(SCRIPT_DIR)
SCHEMA_PATH = os.path.join(PROJECT_ROOT, "schema/settlement_unified.cql")

def run_command(cmd, shell=True, ignore_errors=False):
    try:
        # Capture stderr to STDOUT so we can print it on failure
        subprocess.check_output(cmd, shell=shell, stderr=subprocess.STDOUT)
    except subprocess.CalledProcessError as e:
        if not ignore_errors:
            print(f"Error running command: {cmd}")
            print(f"Output:\n{e.output.decode()}")
            sys.exit(1)

def print_step(msg):
    print(f"\n\033[94m[STEP] {msg}\033[0m")

def print_success(msg):
    print(f"\033[92m[PASS] {msg}\033[0m")

def print_error(msg):
    print(f"\033[91m[FAIL] {msg}\033[0m")

def clean_environment():
    print_step("Cleaning Environment")
    run_command("pkill -f matching_engine_server", ignore_errors=True)
    run_command("pkill -f order_gate_server", ignore_errors=True)
    run_command("pkill -f settlement_service", ignore_errors=True)

    # DB
    print("Resetting Database...")
    run_command(f'docker exec scylla cqlsh -e "DROP KEYSPACE IF EXISTS {KEYSPACE};"')
    run_command(f'docker exec scylla cqlsh -e "CREATE KEYSPACE {KEYSPACE} WITH replication = {{\'class\': \'SimpleStrategy\', \'replication_factor\': 1}};"')

    # Use quotes around path to handle potential spaces
    run_command(f'docker exec -i scylla cqlsh -k {KEYSPACE} < "{SCHEMA_PATH}"')

    # Kafka
    print("Resetting Kafka Topics...")
    run_command("docker exec redpanda rpk topic delete orders trades balance.operations", ignore_errors=True)
    run_command("docker exec redpanda rpk topic create orders trades balance.operations -p 1 -r 1")
    print("Waiting 5s for Kafka metadata propagation...")
    time.sleep(5)

def start_services():
    print_step("Starting Services")

    log_dir = os.path.join(PROJECT_ROOT, "logs")
    os.makedirs(log_dir, exist_ok=True)

    # Using subprocess.Popen to start in background
    with open(os.path.join(log_dir, "matching_engine.log"), "w") as f_me, \
         open(os.path.join(log_dir, "order_gateway.log"), "w") as f_gw, \
         open(os.path.join(log_dir, "settlement.log"), "w") as f_st:

        # Set cwd=PROJECT_ROOT to ensure binaries find config files correctly
        p_me = subprocess.Popen([os.path.join(PROJECT_ROOT, "target/debug/matching_engine_server")], stdout=f_me, stderr=f_me, cwd=PROJECT_ROOT)
        print(f"Matching Engine PID: {p_me.pid}")
        time.sleep(2)

        p_gw = subprocess.Popen([os.path.join(PROJECT_ROOT, "target/debug/order_gate_server")], stdout=f_gw, stderr=f_gw, cwd=PROJECT_ROOT)
        print(f"Order Gateway PID: {p_gw.pid}")
        time.sleep(2)

        p_st = subprocess.Popen([os.path.join(PROJECT_ROOT, "target/debug/settlement_service")], stdout=f_st, stderr=f_st, cwd=PROJECT_ROOT)
        print(f"Settlement Service PID: {p_st.pid}")
        time.sleep(2)

        return [p_me, p_gw, p_st]

def inject_deposits():
    print_step("Injecting Deposits")
    ts = int(time.time() * 1000)

    # User 1001 (Seller): 1.0 BTC (Asset 1)
    dep1 = {
        "type": "TransferIn",
        "data": {
            "request_id": "req-1",
            "user_id": 1001,
            "asset_id": 1,
            "amount": 100_000_000,
            "timestamp": ts
        }
    }

    # User 1002 (Buyer): 20,000 USDT (Asset 2) -> 2,000,000,000,000 (Assuming 8 decimals)
    dep2 = {
        "type": "TransferIn",
        "data": {
            "request_id": "req-2",
            "user_id": 1002,
            "asset_id": 2,
            "amount": 2_000_000_000_000,
            "timestamp": ts
        }
    }

    # Use subprocess to pipe to rpk
    p = subprocess.Popen(["docker", "exec", "-i", "redpanda", "rpk", "topic", "produce", "balance.operations"], stdin=subprocess.PIPE)
    p.communicate(input=json.dumps(dep1).encode() + b"\n" + json.dumps(dep2).encode() + b"\n")

    print("Deposits sent. Waiting 3s...")
    time.sleep(3)

def place_order(user_id, side, price, qty):
    # Use underscores in CID to pass alphanumeric check
    cid = f"{side.lower()}_{int(time.time())}_ORDER_{user_id}"
    order = {
        "cid": cid,
        "symbol": "BTC_USDT",
        "side": side,
        "order_type": "Limit",
        "price": price,
        "quantity": qty
    }

    url = f"{GATEWAY_URL}/api/orders?user_id={user_id}"
    req = urllib.request.Request(url, data=json.dumps(order).encode(), headers={'Content-Type': 'application/json'}, method='POST')

    try:
        with urllib.request.urlopen(req) as response:
            data = json.load(response)
            # ApiResponse { status: 0, msg: "ok", data: ... }
            if data.get("status") == 0:
                 print(f"Order Placed: {side} {qty} @ {price} (CID: {cid})")
                 return True
            else:
                 print_error(f"Order Failed: {data}")
                 return False
    except urllib.error.HTTPError as e:
        print_error(f"HTTP Error {e.code}: {e.read().decode()}")
        return False
    except urllib.error.URLError as e:
         print_error(f"Connection Error: {e}")
         return False
    except Exception as e:
        print_error(f"Request Error: {e}")
        return False

def verify_state():
    print_step("Verifying State")

    def query(cql):
        cmd = f'docker exec scylla cqlsh -k {KEYSPACE} -e "{cql}"'
        try:
            return subprocess.check_output(cmd, shell=True).decode()
        except subprocess.CalledProcessError:
            return ""

    # 1. Active Orders (Should be Empty)
    active = query("SELECT * FROM active_orders")
    if "(0 rows)" in active:
        print_success("Active Orders Empty (Correct)")
    else:
        print_error(f"Active Orders NOT Empty:\n{active}")

    # 2. Settled Trades (Should have 1)
    trades = query("SELECT trade_id, price, quantity FROM settled_trades")
    if "(1 rows)" in trades or "1 rows" in trades:
        print_success("Settled Trades Found (Correct)")
        print(trades)
    else:
        # Check rows count manually
        lines = [l for l in trades.split('\n') if l.strip()]
        if len(lines) > 4: # Header + Separator + Row + RowCount
             print_success("Settled Trades Found")
             print(trades)
        else:
             print_error(f"Settled Trades Missing:\n{trades}")

    # 3. Order History (Should have 2 filled orders)
    history = query("SELECT order_id, side, status, filled_qty FROM order_history")
    if "Filled" in history:
        print_success("Order History shows FILLED orders")
        print(history)
    else:
        print_error(f"Order History Incorrect:\n{history}")

    # 4. Balances
    # User 1001: Should have less BTC, more USDT
    # User 1002: Should have more BTC, less USDT
    balances = query("SELECT * FROM user_balances")
    print("Final Balances:")
    print(balances)

def main():
    try:
        clean_environment()
        ps = start_services()

        inject_deposits()

        print_step("Placing Orders")
        # Sell 0.5 BTC @ 150.0
        place_order(1001, "Sell", "150.0", "0.5")
        time.sleep(1)
        # Buy 0.5 BTC @ 150.0
        place_order(1002, "Buy", "150.0", "0.5")

        print("Waiting 5s for settlement...")
        time.sleep(5)

        verify_state()

    finally:
        print_step("Cleaning Up")
        run_command("pkill -f matching_engine_server", ignore_errors=True)
        run_command("pkill -f order_gate_server", ignore_errors=True)
        run_command("pkill -f settlement_service", ignore_errors=True)

if __name__ == "__main__":
    main()
