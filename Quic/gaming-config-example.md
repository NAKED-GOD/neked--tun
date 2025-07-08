# Gaming Configuration Examples

This file contains optimized configurations for gaming scenarios with rstun.

## Quick Gaming Setup

### Server (rstund)
```bash
# Basic gaming server with optimized settings
rstund \
  --gaming-mode \
  --addr 0.0.0.0:6060 \
  --password your_secure_password \
  --quic-timeout-ms 15000 \
  --tcp-timeout-ms 10000 \
  --udp-timeout-ms 2000 \
  --workers 4
```

### Client (rstunc)
```bash
# Gaming client with optimized settings
rstunc \
  --gaming-mode \
  --server-addr your_server_ip:6060 \
  --password your_secure_password \
  --tcp-mappings "OUT^0.0.0.0:7777^7777" \
  --udp-mappings "OUT^0.0.0.0:7777^7777" \
  --quic-timeout-ms 15000 \
  --tcp-timeout-ms 10000 \
  --udp-timeout-ms 2000 \
  --wait-before-retry-ms 2000 \
  --workers 4 \
  --cipher chacha20-poly1305
```

## Popular Game Configurations

### Counter-Strike 2
```bash
# Server
rstund \
  --gaming-mode \
  --addr 0.0.0.0:6060 \
  --password cs2_password \
  --tcp-upstream 27015 \
  --udp-upstream 27015

# Client
rstunc \
  --gaming-mode \
  --server-addr your_server:6060 \
  --password cs2_password \
  --tcp-mappings "OUT^0.0.0.0:27015^27015" \
  --udp-mappings "OUT^0.0.0.0:27015^27015"
```

### Valorant
```bash
# Server
rstund \
  --gaming-mode \
  --addr 0.0.0.0:6060 \
  --password valorant_password

# Client
rstunc \
  --gaming-mode \
  --server-addr your_server:6060 \
  --password valorant_password \
  --tcp-mappings "OUT^0.0.0.0:7777^7777" \
  --udp-mappings "OUT^0.0.0.0:7777^7777"
```

### League of Legends
```bash
# Server
rstund \
  --gaming-mode \
  --addr 0.0.0.0:6060 \
  --password lol_password

# Client
rstunc \
  --gaming-mode \
  --server-addr your_server:6060 \
  --password lol_password \
  --tcp-mappings "OUT^0.0.0.0:8393^8393" \
  --udp-mappings "OUT^0.0.0.0:8393^8393"
```

## Performance Tuning Tips

### For Maximum Performance:
1. **Use gaming mode**: Always enable `--gaming-mode` for gaming
2. **Optimize worker threads**: Use 2-4 workers for most gaming scenarios
3. **Use ChaCha20-Poly1305**: Fastest cipher for gaming
4. **Reduce timeouts**: Use shorter timeouts for faster reconnection
5. **Monitor network**: Use `--loglevel D` for debugging

### Memory Usage Comparison:
- **Normal mode**: ~50MB RAM per connection
- **Gaming mode**: ~25MB RAM per connection (50% reduction)

### Latency Improvements:
- **Buffer size reduction**: 50% smaller buffers for faster processing
- **Faster timeouts**: Quicker recovery from network issues
- **Optimized QUIC settings**: Better for real-time traffic

## Troubleshooting

### High Ping Issues:
1. Check server location - closer is better
2. Use gaming mode: `--gaming-mode`
3. Reduce worker threads if CPU is overloaded
4. Use faster cipher: `--cipher chacha20-poly1305`

### Connection Drops:
1. Increase timeout values slightly
2. Check network stability
3. Use `--wait-before-retry-ms 1000` for faster reconnection

### Memory Issues:
1. Enable gaming mode for 50% memory reduction
2. Reduce number of concurrent tunnels
3. Monitor with `--loglevel D` 