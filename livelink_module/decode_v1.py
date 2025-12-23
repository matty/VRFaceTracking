import struct

data = bytes([1, 0, 36, 0, 53, 57, 55, 70, 67, 68, 67, 69, 45, 53, 49, 53, 52, 45, 52, 52, 70, 50, 45, 66, 51, 55, 56, 45, 66, 67, 67, 68, 70, 51, 52, 68, 53, 70, 68, 67, 36, 115, 43, 0, 0, 178, 233, 61, 60, 0, 0, 0, 1, 0, 0, 0, 251, 0, 112, 45, 32, 43, 80, 9, 176, 7, 224, 4, 16, 8, 144, 12, 0, 34, 0, 0, 0, 0, 192, 99, 0, 71, 0, 0, 0, 0, 0, 0, 0, 0, 160, 111, 192, 77, 176, 77, 160, 75, 176, 2])

print(f"Total length: {len(data)}")

offset = 0
version = data[offset]
print(f"Version: {version}")
offset += 1

# Try u16 BE length
try:
    val = struct.unpack('>H', data[offset:offset+2])[0]
    print(f"u16 BE at 1: {val} (Expected 36?)")
    offset += 2
    # Next byte is 0. Padding?
    padding = data[offset]
    print(f"Padding byte: {padding}")
    offset += 1
    
    # Now read 36 bytes
    did_str = data[offset:offset+36].decode('utf-8', errors='ignore')
    print(f"Device ID: {did_str}")
    offset += 36
    
    print(f"Offset after Device ID: {offset}")
    print(f"Bytes at offset: {list(data[offset:offset+10])}")
    
    # Look at floats in the data
    print("\n--- Searching for Floats ---")
    for i in range(offset, len(data)-4):
        chunk = data[i:i+4]
        try:
            f_be = struct.unpack('>f', chunk)[0]
            f_le = struct.unpack('<f', chunk)[0]
            if 0.0 <= abs(f_be) <= 1.0 or 0.0 <= abs(f_le) <= 1.0:
                print(f"Offset {i}: BE={f_be:.4f}, LE={f_le:.4f} Bytes={list(chunk)}")
        except:
            pass

except Exception as e:
    print(f"Error: {e}")
