#!/bin/bash
set -e

# Always create required directories at the start in the root of the current working directory
ROOT_DIR="$(pwd)/hysteria-setup"
CERTS_DIR="$ROOT_DIR/certs"
CONFIGS_DIR="$ROOT_DIR/configs"
BIN_PATH="$ROOT_DIR/hysteria"

mkdir -p "$CERTS_DIR" "$CONFIGS_DIR"

GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

function install_dependencies() {
    echo -e "${GREEN}Checking and installing dependencies...${NC}"
    if ! command -v certbot >/dev/null 2>&1; then
        echo "Installing certbot..."
        if command -v apt-get >/dev/null 2>&1; then
            sudo apt-get update
            sudo apt-get install -y certbot
        elif command -v yum >/dev/null 2>&1; then
            sudo yum install -y certbot
        elif command -v dnf >/dev/null 2>&1; then
            sudo dnf install -y certbot
        else
            echo -e "${RED}No supported package manager found. Please install certbot manually.${NC}"
        fi
    else
        echo "certbot is already installed."
    fi
    # Install openssl if not present
    if ! command -v openssl >/dev/null 2>&1; then
        echo "Installing openssl..."
        if command -v apt-get >/dev/null 2>&1; then
            sudo apt-get install -y openssl
        elif command -v yum >/dev/null 2>&1; then
            sudo yum install -y openssl
        elif command -v dnf >/dev/null 2>&1; then
            sudo dnf install -y openssl
        else
            echo -e "${RED}No supported package manager found. Please install openssl manually.${NC}"
        fi
    else
        echo "openssl is already installed."
    fi
}

function check_port() {
    local port=$1
    if command -v netstat >/dev/null 2>&1; then
        netstat -tuln | grep -q ":$port "
    elif command -v ss >/dev/null 2>&1; then
        ss -tuln | grep -q ":$port "
    else
        # Fallback: try to bind to the port
        timeout 1 bash -c "echo >/dev/tcp/127.0.0.1/$port" 2>/dev/null && return 1 || return 0
    fi
}

function get_available_port() {
    local port_type=$1
    local port
    local max_attempts=5
    local attempts=0
    
    while [ $attempts -lt $max_attempts ]; do
        read -p "Enter $port_type: " port
        
        # Basic validation
        if [[ ! "$port" =~ ^[0-9]+$ ]] || [ "$port" -lt 1 ] || [ "$port" -gt 65535 ]; then
            echo -e "${RED}Invalid port number. Please enter a number between 1-65535.${NC}"
            ((attempts++))
            continue
        fi
        
        if check_port "$port"; then
            echo -e "${GREEN}Port $port is available.${NC}"
            echo "$port"
            return 0
        else
            echo -e "${RED}Port $port is already in use. Please choose a different port.${NC}"
            ((attempts++))
        fi
    done
    
    echo -e "${RED}Too many failed attempts. Using port $port anyway.${NC}"
    echo "$port"
    return 0
}

function download_hysteria() {
    if [ ! -f "$BIN_PATH" ]; then
        echo "Installing Hysteria using official installer..."
        mkdir -p "$ROOT_DIR"
        
        # Use the official Hysteria installer automatically
        bash <(curl -fsSL https://get.hy2.sh/)
        
        # Check if binary was installed in system path and copy it
        if command -v hysteria >/dev/null 2>&1; then
            cp $(which hysteria) "$BIN_PATH"
            chmod +x "$BIN_PATH"
            echo -e "${GREEN}Hysteria installed successfully.${NC}"
        elif [ -f "/usr/local/bin/hysteria" ]; then
            cp /usr/local/bin/hysteria "$BIN_PATH"
            chmod +x "$BIN_PATH"
            echo -e "${GREEN}Hysteria installed successfully.${NC}"
        else
            echo -e "${RED}Failed to install Hysteria. Please check your internet connection.${NC}"
            exit 1
        fi
    fi
}

function create_project_structure() {
    mkdir -p "$CERTS_DIR" "$CONFIGS_DIR"
}

function open_ports() {
    local PORTS_CSV="$1"
    IFS=',' read -ra PORTS <<< "$PORTS_CSV"
    for port in "${PORTS[@]}"; do
        port=$(echo "$port" | tr -d ' ')
        if [[ "$port" =~ ^[0-9]+$ ]] && [ "$port" -ge 1 ] && [ "$port" -le 65535 ]; then
            echo -e "${GREEN}Opening port $port (TCP/UDP) in firewall...${NC}"
            if command -v ufw >/dev/null 2>&1; then
                sudo ufw allow $port
                sudo ufw allow $port/udp
            fi
            if command -v iptables >/dev/null 2>&1; then
                sudo iptables -C INPUT -p tcp --dport $port -j ACCEPT 2>/dev/null || sudo iptables -A INPUT -p tcp --dport $port -j ACCEPT
                sudo iptables -C INPUT -p udp --dport $port -j ACCEPT 2>/dev/null || sudo iptables -A INPUT -p udp --dport $port -j ACCEPT
            fi
        fi
    done
    # Reload ufw if active
    if command -v ufw >/dev/null 2>&1 && sudo ufw status | grep -q active; then
        sudo ufw reload
    fi
}

function check_tunnel_status() {
    echo -e "${GREEN}--- Tunnel Status Check ---${NC}"
    local configs=("$CONFIGS_DIR/server.yaml" "$CONFIGS_DIR/client.yaml")
    for config in "${configs[@]}"; do
        if [ -f "$config" ]; then
            echo "Checking: $(basename "$config")"
            local ports=()
            # Extract forwarded ports from config
            while read -r line; do
                if [[ "$line" =~ local:\ 127\.0\.0\.1:([0-9]+) ]]; then
                    ports+=("${BASH_REMATCH[1]}")
                fi
            done < "$config"
            if [ ${#ports[@]} -eq 0 ]; then
                echo "  No forwarded ports found."
                continue
            fi
            for port in "${ports[@]}"; do
                # Try to connect to the port
                timeout 2 bash -c "</dev/tcp/127.0.0.1/$port" 2>/dev/null && \
                    echo "  Port $port: UP" || \
                    echo "  Port $port: DOWN"
            done
        fi
    done
}

function create_server() {
    echo -e "${GREEN}--- Iran Server Configuration (server.yaml) ---${NC}"
    read -p "Enter a valid domain or IP for the Iran server: " DOMAIN
    read -p "Enter TUN PORT (for Hysteria tunnel): " TUN_PORT
    read -p "Enter ports to forward (separated by comma, e.g. 22,80,443): " FORWARD_PORTS
    read -p "Strong password for the tunnel: " PASSWORD
    read -p "Security token (for tunnel authentication): " TOKEN

    mkdir -p "$CERTS_DIR"
    CERT_PATH="$CERTS_DIR/cert.pem"
    KEY_PATH="$CERTS_DIR/key.pem"
    LETSENCRYPT_CERT="/etc/letsencrypt/live/$DOMAIN/fullchain.pem"
    LETSENCRYPT_KEY="/etc/letsencrypt/live/$DOMAIN/privkey.pem"

    # If certbot certs already exist, just copy them
    if [ -f "$LETSENCRYPT_CERT" ] && [ -f "$LETSENCRYPT_KEY" ]; then
        echo -e "${GREEN}Existing certificate found for $DOMAIN. Copying to certs directory...${NC}"
        cp "$LETSENCRYPT_CERT" "$CERT_PATH"
        cp "$LETSENCRYPT_KEY" "$KEY_PATH"
    else
        # Try to obtain a real certificate for the domain using certbot
        echo "Attempting to obtain a real certificate for $DOMAIN using certbot..."
        if command -v certbot >/dev/null 2>&1; then
            certbot certonly --standalone --preferred-challenges http --non-interactive --agree-tos --register-unsafely-without-email -d "$DOMAIN"
            # Always try to copy certbot files after running certbot
            if [ -f "$LETSENCRYPT_CERT" ] && [ -f "$LETSENCRYPT_KEY" ]; then
                cp "$LETSENCRYPT_CERT" "$CERT_PATH"
                cp "$LETSENCRYPT_KEY" "$KEY_PATH"
                echo -e "${GREEN}Certificate files copied to $CERTS_DIR.${NC}"
            else
                echo -e "${RED}Failed to find certbot certificate files. Falling back to self-signed certificate.${NC}"
                openssl req -x509 -newkey rsa:2048 -keyout "$KEY_PATH" -out "$CERT_PATH" -days 365 -nodes -subj "/CN=$DOMAIN"
            fi
        else
            echo -e "${RED}certbot not found. Generating a self-signed certificate.${NC}"
            openssl req -x509 -newkey rsa:2048 -keyout "$KEY_PATH" -out "$CERT_PATH" -days 365 -nodes -subj "/CN=$DOMAIN"
        fi
    fi

    # Check that cert.pem and key.pem exist
    if [ ! -f "$CERT_PATH" ] || [ ! -f "$KEY_PATH" ]; then
        echo -e "${RED}ERROR: Certificate or key file was not created!${NC}"
        echo "Expected: $CERT_PATH and $KEY_PATH"
        ls -l "$CERTS_DIR"
        exit 1
    fi

    # Create server.yaml with performance optimizations
    cat > "$CONFIGS_DIR/server.yaml" <<EOF
listen: :$TUN_PORT
auth:
  type: password
  password: "$PASSWORD"
tls:
  cert: "/root/hysteria-setup/certs/cert.pem"
  key: "/root/hysteria-setup/certs/key.pem"
forwarders:
EOF

    # Add forwarders for each port
    IFS=',' read -ra PORTS <<< "$FORWARD_PORTS"
    for port in "${PORTS[@]}"; do
        port=$(echo "$port" | tr -d ' ')
        if [[ "$port" =~ ^[0-9]+$ ]] && [ "$port" -ge 1 ] && [ "$port" -le 65535 ]; then
            echo "  - local: 127.0.0.1:$port" >> "$CONFIGS_DIR/server.yaml"
            echo "    remote: 127.0.0.1:$port" >> "$CONFIGS_DIR/server.yaml"
        fi
    done

    cat >> "$CONFIGS_DIR/server.yaml" <<EOF

# Performance optimizations for maximum speed
bandwidth:
  up: "1000 mbps"
  down: "1000 mbps"
buffer:
  size: 16777216
  max_connections: 1000
quic:
  max_idle_timeout: 24h
  max_incoming_streams: 100
  max_incoming_uni_streams: 100
  keepalive: 10s
network:
  tcp_fast_open: true
  tcp_congestion: bbr
  udp_gso: true
log:
  level: warn
  timestamp: false
EOF

    echo -e "${GREEN}configs/server.yaml created with performance optimizations.${NC}"
    echo -e "${GREEN}Starting Hysteria server...${NC}"
    "$BIN_PATH" server -c "$CONFIGS_DIR/server.yaml" &
    echo -e "${GREEN}Hysteria server is running.${NC}"

    open_ports "$TUN_PORT,$FORWARD_PORTS"
}

function create_client() {
    echo -e "${GREEN}--- Foreign Server Configuration (client.yaml) ---${NC}"
    read -p "Enter the IP or domain of the Iran server: " SERVER_IP
    read -p "Enter TUN PORT (same as Iran server): " TUN_PORT
    read -p "Enter ports to forward (separated by comma, e.g. 22,80,443): " FORWARD_PORTS
    read -p "Tunnel password (same as Iran server): " PASSWORD
    read -p "Security token (same as Iran server): " TOKEN
    
    # Create client.yaml with performance optimizations
    cat > "$CONFIGS_DIR/client.yaml" <<EOF
server: "$SERVER_IP:$TUN_PORT"
auth: "$PASSWORD"
tls:
  insecure: true
forwarders:
EOF

    # Add forwarders for each port
    IFS=',' read -ra PORTS <<< "$FORWARD_PORTS"
    for port in "${PORTS[@]}"; do
        port=$(echo "$port" | tr -d ' ')
        if [[ "$port" =~ ^[0-9]+$ ]] && [ "$port" -ge 1 ] && [ "$port" -le 65535 ]; then
            echo "  - local: 127.0.0.1:$port" >> "$CONFIGS_DIR/client.yaml"
            echo "    remote: 127.0.0.1:$port" >> "$CONFIGS_DIR/client.yaml"
        fi
    done

    cat >> "$CONFIGS_DIR/client.yaml" <<EOF

# Performance optimizations for maximum speed
bandwidth:
  up: "1000 mbps"
  down: "1000 mbps"
buffer:
  size: 16777216
quic:
  max_idle_timeout: 24h
  max_incoming_streams: 100
  max_incoming_uni_streams: 100
  keepalive: 10s
network:
  tcp_fast_open: true
  tcp_congestion: bbr
  udp_gso: true
socks5:
  listen: 127.0.0.1:1080
log:
  level: warn
  timestamp: false
EOF

    echo -e "${GREEN}configs/client.yaml created with performance optimizations.${NC}"
    echo -e "${GREEN}Starting Hysteria client...${NC}"
    "$BIN_PATH" client -c "$CONFIGS_DIR/client.yaml" &
    echo -e "${GREEN}Hysteria client is running.${NC}"

    open_ports "$TUN_PORT,$FORWARD_PORTS"
}

function uninstall_config() {
    echo -e "${GREEN}--- Uninstall Config ---${NC}"
    local configs=()
    local i=1

    # جستجوی کانفیگ‌ها
    for f in "$CONFIGS_DIR"/*.yaml; do
        [ -e "$f" ] || continue
        configs+=("$f")
        echo "$i) $(basename "$f")"
        ((i++))
    done

    if [ ${#configs[@]} -eq 0 ]; then
        echo "No configs found to uninstall."
        return
    fi

    read -p "Enter the number of the config you want to uninstall: " choice
    if [[ "$choice" =~ ^[0-9]+$ ]] && [ "$choice" -ge 1 ] && [ "$choice" -le "${#configs[@]}" ]; then
        config_to_remove="${configs[$((choice-1))]}"
        echo "Stopping any running Hysteria process for this config (if exists)..."
        pkill -f "$config_to_remove" 2>/dev/null || true
        echo "Deleting $config_to_remove"
        rm -f "$config_to_remove"
        echo -e "${GREEN}Config removed.${NC}"
    else
        echo "Invalid selection."
    fi
}

function menu() {
    while true; do
        echo -e "${GREEN}\n--- Main Menu ---${NC}"
        echo "1) Create Tunnel (generate and run config)"
        echo "2) Exit"
        echo "3) Uninstall Config"
        echo "4) Check Tunnel Status"
        read -p "Your choice: " CHOICE
        case $CHOICE in
            1)
                echo "1) Iran Server (server.yaml)"
                echo "2) Foreign Server (client.yaml)"
                read -p "Which do you want to configure? (1/2): " ROLE
                create_project_structure
                download_hysteria
                if [ "$ROLE" == "1" ]; then
                    create_server
                elif [ "$ROLE" == "2" ]; then
                    create_client
                else
                    echo "Invalid selection!"
                fi
                ;;
            2)
                echo "Exiting..."
                exit 0
                ;;
            3)
                uninstall_config
                ;;
            4)
                check_tunnel_status
                ;;
            *)
                echo "Invalid selection!"
                ;;
        esac
    done
}

# Call this at the start of the script
install_dependencies

menu 