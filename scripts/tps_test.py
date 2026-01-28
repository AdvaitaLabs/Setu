#!/usr/bin/env python3
"""Simple TPS test for Setu validator"""

import requests
import time
import concurrent.futures
import sys
import os
import signal

os.environ['NO_PROXY'] = '127.0.0.1,localhost'
os.environ['http_proxy'] = ''
os.environ['https_proxy'] = ''

URL = "http://127.0.0.1:8080/api/v1/transfer"
TOTAL = int(sys.argv[1]) if len(sys.argv) > 1 else 100
CONCURRENCY = int(sys.argv[2]) if len(sys.argv) > 2 else 50

session = requests.Session()
session.trust_env = False  # Ignore system proxy settings

def send_transfer(i):
    try:
        resp = session.post(URL, json={
            "from": "alice",
            "to": "bob", 
            "amount": 1,
            "transfer_type": "flux",
            "resources": []
        }, timeout=10)
        data = resp.json()
        return data.get("success", False)
    except Exception as e:
        return False

print(f"=== TPS Test: {TOTAL} requests, {CONCURRENCY} concurrency ===")

start = time.time()

success = 0
fail = 0

with concurrent.futures.ThreadPoolExecutor(max_workers=CONCURRENCY) as executor:
    futures = {executor.submit(send_transfer, i): i for i in range(TOTAL)}
    completed = 0
    try:
        for future in concurrent.futures.as_completed(futures, timeout=120):
            completed += 1
            if future.result():
                success += 1
            else:
                fail += 1
            if completed % 50 == 0:
                elapsed = time.time() - start
                print(f"Progress: {completed}/{TOTAL} ({success} ok) - {elapsed:.2f}s")
    except concurrent.futures.TimeoutError:
        print(f"WARNING: Timeout after 120s, completed {completed}/{TOTAL}")

duration = time.time() - start
tps = success / duration if duration > 0 else 0

print(f"\n=== Results ===")
print(f"Total: {TOTAL}")
print(f"Success: {success}")
print(f"Failed: {fail}")
print(f"Duration: {duration:.2f}s")
print(f"TPS: {tps:.2f}")
