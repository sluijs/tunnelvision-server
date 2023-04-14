#!/usr/bin/env python

import asyncio
import websockets
import numpy as np
import json


async def hello():
    uri = "ws://localhost:8765/ws"
    async with websockets.connect(uri) as websocket:
        # name = input("What's your name? ")
        # await websocket.send(name)
        # print(f">>> {name}")

        # Define the array we want to send
        arr = np.random.randint(0, 2048, (25, 1, 512, 512, 1), dtype=np.uint16)

        # Send the header first
        header = "JSON" + json.dumps({"shape": arr.shape, "dtype": arr.dtype.name})
        header = header.encode("utf-8")

        await websocket.send(header)

        # Send the array
        await websocket.send(arr.tobytes())

        # for chunk in np.array_split(arr, arr.shape[0], axis=0):
        #     print("Sending chunk...")
        #     await websocket.send(chunk.tobytes())

        # await websocket.send(arr.tobytes())
        # greeting = await websocket.recv()
        # print(f"<<< {greeting}")

        await websocket.close(reason="Goodbye!")


if __name__ == "__main__":
    asyncio.run(hello())