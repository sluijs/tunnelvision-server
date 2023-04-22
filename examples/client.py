#!/usr/bin/env python

import asyncio
import json

import numpy as np
import websockets
from shortuuid import uuid
import argparse


async def hello(host: str, port: int, hash: str = "dev"):
    uri = f"ws://{host}:{port}/ws"
    async with websockets.connect(uri) as websocket:
        # Define the array we want to send
        arr = np.random.randint(0, 2048, (25, 1, 512, 512, 1), dtype=np.uint16)

        # Send the header first
        msg = json.dumps({"shape": arr.shape, "dtype": arr.dtype.name, "hash": hash})
        await websocket.send(msg)

        # Send the array
        await websocket.send(hash.encode("utf-8") + arr.tobytes())

        # for chunk in np.array_split(arr, arr.shape[0], axis=0):
        #     print("Sending chunk...")
        #     await websocket.send(chunk.tobytes())

        # await websocket.send(arr.tobytes())
        # greeting = await websocket.recv()
        # print(f"<<< {greeting}")

        await websocket.close(reason="Goodbye!")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", type=str, default="localhost")
    parser.add_argument("--port", type=int, default=8765)
    parser.add_argument("--hash", type=str, default="dev")
    args = parser.parse_args()

    asyncio.run(asyncio.wait_for(hello(args.host, args.port, args.hash), timeout=5))
