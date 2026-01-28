#!/usr/bin/env python3
"""Simple TPS Test for Setu Validator"""
import requests
import time
import concurrent.futures
import sys

URL = "http://127.0.0.1:8080/api/v1/transfer"
DATA = {"from":"alice","to":"bob","amount":1,"transfer_type":"flux","resources":[]}

def send_request():
    try:
        s = requests.Session()
        s.trust_env = False
        resp = s.post(URL, json=DATA, timeout=30)
        return resp.status_code == 200 and resp.json().get("success", False)
    except:
        return False

def test_tps(total, workers):
    success = 0
    start = time.time()
    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as executor:
        results = list(executor.map(lambda _: send_request(), range(total)))
    elapsed = time.time() - start
    success = sum(results)
    tps = total / elapsed if elapsed > 0 else 0
    print(f"\n=== TPS Test Results ===")
    print(f"Total requests: {total}")
    print(f"Successful: {success}")
    print(f"Failed: {total - success}")
    print(f"Time: {elapsed:.2f}s")
    print(f"TPS: {tps:.2f}")
    print(f"Success rate: {success/total*100:.1f}%")
    return tps

if __name__ == "__main__":
    total = int(sys.argv[1]) if len(sys.argv) > 1 else 100
    workers = int(sys.argv[2]) if len(sys.argv) > 2 else 50
    test_tps(total, workers)
