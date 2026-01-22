# Setu Docker Deployment

This directory contains Docker deployment configurations and scripts for the Setu project.

## Directory Structure

```
docker/
├── Dockerfile.validator          # Validator node Dockerfile
├── Dockerfile.solver             # Solver node Dockerfile
├── docker-compose.yml            # Single validator + multi-solver setup
├── docker-compose.multi-validator.yml  # Multi-validator setup (for consensus testing)
├── .dockerignore                 # Docker build ignore file
├── .env.example                  # Environment variables example
├── scripts/                      # Docker management scripts
│   ├── build.sh                 # Build images
│   ├── start.sh                 # Start services
│   ├── stop.sh                  # Stop services
│   └── logs.sh                  # View logs
└── README.md                     # This document
```

## Quick Start

### 1. Prerequisites

Ensure you have installed:
- Docker (>= 20.10)
- Docker Compose (>= 2.0)

### 2. Build Images

```bash
cd docker
./scripts/build.sh
```

Or manually:

```bash
# Build Validator image
docker build -f docker/Dockerfile.validator -t setu-validator:latest .

# Build Solver image
docker build -f docker/Dockerfile.solver -t setu-solver:latest .
```

### 3. Start Services

#### Option 1: Using Scripts (Recommended)

```bash
cd docker
./scripts/start.sh
```

#### Option 2: Using docker-compose

```bash
# Start single validator + single solver
docker-compose -f docker/docker-compose.yml up -d

# Start single validator + multiple solvers
docker-compose -f docker/docker-compose.yml --profile multi-solver up -d

# Start multi-validator (test consensus)
docker-compose -f docker/docker-compose.multi-validator.yml up -d
```

### 4. Verify Services

```bash
# Check service status
docker-compose -f docker/docker-compose.yml ps

# Check Validator health
curl http://localhost:8080/api/v1/health

# List registered Solvers
curl http://localhost:8080/api/v1/solvers
```

### 5. View Logs

```bash
# Using script
cd docker
./scripts/logs.sh

# Or using docker-compose
docker-compose -f docker/docker-compose.yml logs -f

# View specific service logs
docker-compose -f docker/docker-compose.yml logs -f validator
docker-compose -f docker/docker-compose.yml logs -f solver-1
```

### 6. Stop Services

```bash
# Using script
cd docker
./scripts/stop.sh

# Or using docker-compose
docker-compose -f docker/docker-compose.yml down

# Stop and remove volumes
docker-compose -f docker/docker-compose.yml down -v
```

## Configuration

### Environment Variables

Create `.env` file to customize configuration:

```bash
cp .env.example .env
# Edit .env file
```

Key environment variables:

#### Validator Configuration
- `VALIDATOR_ID`: Validator ID (default: validator-1)
- `VALIDATOR_HTTP_PORT`: HTTP API port (default: 8080)
- `VALIDATOR_P2P_PORT`: P2P network port (default: 8081)
- `VALIDATOR_LISTEN_ADDR`: Listen address (default: 0.0.0.0)
- `IS_LEADER`: Whether this is the leader (default: true)
- `DB_PATH`: Database path (default: /data/db)

#### Solver Configuration
- `SOLVER_ID`: Solver ID (default: solver-1)
- `SOLVER_PORT`: Solver RPC port (default: 9001)
- `SOLVER_CAPACITY`: Solver capacity (default: 100)
- `VALIDATOR_ADDRESS`: Validator address (default: validator)
- `AUTO_REGISTER`: Auto-register to Validator (default: true)

#### Logging Configuration
- `RUST_LOG`: Log level (default: info)

### Port Mapping

#### Single Validator Mode
- `8080`: Validator HTTP API
- `8081`: Validator P2P Network
- `9001`: Solver-1 RPC
- `9002`: Solver-2 RPC (with multi-solver profile)

#### Multi-Validator Mode
- `8080`: Validator-1 HTTP API
- `8081`: Validator-1 P2P
- `8082`: Validator-2 HTTP API
- `8083`: Validator-2 P2P
- `8084`: Validator-3 HTTP API
- `8085`: Validator-3 P2P
- `9001`: Solver-1 RPC

### Data Persistence

Volume mounts:
- `validator-data`: Validator data (database and logs)
- `solver-1-data`: Solver-1 data (logs)
- `solver-2-data`: Solver-2 data (logs)

View volumes:
```bash
docker volume ls | grep setu
```

Backup volume:
```bash
docker run --rm -v validator-data:/data -v $(pwd):/backup alpine tar czf /backup/validator-backup.tar.gz /data
```

Restore volume:
```bash
docker run --rm -v validator-data:/data -v $(pwd):/backup alpine tar xzf /backup/validator-backup.tar.gz -C /
```

## Deployment Scenarios

### Scenario 1: Development (Single Node)

Simplest setup for local development:

```bash
docker-compose -f docker/docker-compose.yml up -d
```

Includes:
- 1 Validator
- 1 Solver

### Scenario 2: Testing (Multi-Solver)

Test load balancing and routing:

```bash
docker-compose -f docker/docker-compose.yml --profile multi-solver up -d
```

Includes:
- 1 Validator
- 2 Solvers

### Scenario 3: Consensus Testing (Multi-Validator)

Test consensus mechanism and leader election:

```bash
docker-compose -f docker/docker-compose.multi-validator.yml up -d
```

Includes:
- 3 Validators (forming consensus network)
- 1 Solver

### Scenario 4: Production

Production recommendations:

1. **External Database**: Mount RocksDB data to host or network storage
2. **Monitoring**: Integrate Prometheus + Grafana
3. **Load Balancing**: Add Nginx/HAProxy in front of Validators
4. **Log Collection**: Use ELK or Loki for log aggregation
5. **Backup Strategy**: Regular volume backups

Example production config (needs to be created):
```bash
docker-compose -f docker/docker-compose.prod.yml up -d
```

## Common Issues

### 1. Container Won't Start

Check logs:
```bash
docker-compose -f docker/docker-compose.yml logs validator
```

Common causes:
- Port already in use: Modify port mapping in docker-compose.yml
- Permission issues: Ensure volumes have correct permissions
- Out of memory: Increase Docker memory limit

### 2. Solver Can't Register to Validator

Check:
```bash
# Check network connectivity
docker-compose -f docker/docker-compose.yml exec solver-1 ping validator

# Check if Validator is ready
curl http://localhost:8080/api/v1/health
```

### 3. Image Build Fails

Common causes:
- Network issues: Configure Rust mirror
- Dependency issues: Ensure all dependencies are in Cargo.toml

Configure Rust mirror (add to Dockerfile):
```dockerfile
RUN mkdir -p ~/.cargo && \
    echo '[source.crates-io]' > ~/.cargo/config.toml && \
    echo 'replace-with = "ustc"' >> ~/.cargo/config.toml && \
    echo '[source.ustc]' >> ~/.cargo/config.toml && \
    echo 'registry = "https://mirrors.ustc.edu.cn/crates.io-index"' >> ~/.cargo/config.toml
```

### 4. Performance Optimization

Optimization tips:
- Use `--release` build (already enabled)
- Adjust Docker resource limits
- Use SSD for volume storage
- Enable Docker BuildKit for faster builds

Enable BuildKit:
```bash
export DOCKER_BUILDKIT=1
docker build -f docker/Dockerfile.validator -t setu-validator:latest .
```

## Monitoring and Maintenance

### Health Checks

All services have health checks configured:

```bash
# View health status
docker-compose -f docker/docker-compose.yml ps

# Manual health check
curl http://localhost:8080/api/v1/health
```

### Resource Monitoring

```bash
# View container resource usage
docker stats

# View specific containers
docker stats setu-validator setu-solver-1
```

### Log Management

Configure log rotation (add to docker-compose.yml):

```yaml
services:
  validator:
    logging:
      driver: "json-file"
      options:
        max-size: "10m"
        max-file: "3"
```

### Update Services

```bash
# Rebuild images
docker-compose -f docker/docker-compose.yml build

# Restart services (without losing data)
docker-compose -f docker/docker-compose.yml up -d --force-recreate

# Rolling update (production)
docker-compose -f docker/docker-compose.yml up -d --no-deps --build validator
```

## Security Recommendations

1. **Don't use default keys in production**
2. **Limit container privileges**: Run as non-root user (already configured)
3. **Network isolation**: Use Docker networks to isolate services
4. **Regular updates**: Keep base images and dependencies up to date
5. **Monitor logs**: Watch for abnormal access and errors

## Troubleshooting

### View Detailed Logs

```bash
# View all service logs
docker-compose -f docker/docker-compose.yml logs -f

# View last 100 lines
docker-compose -f docker/docker-compose.yml logs --tail=100

# View specific time range
docker-compose -f docker/docker-compose.yml logs --since 2024-01-01T00:00:00
```

### Enter Container for Debugging

```bash
# Enter Validator container
docker-compose -f docker/docker-compose.yml exec validator bash

# Enter Solver container
docker-compose -f docker/docker-compose.yml exec solver-1 bash
```

### Network Debugging

```bash
# View network configuration
docker network inspect docker_setu-network

# Test inter-container connectivity
docker-compose -f docker/docker-compose.yml exec solver-1 ping validator
```

## References

- [Docker Documentation](https://docs.docker.com/)
- [Docker Compose Documentation](https://docs.docker.com/compose/)
- [Rust Docker Best Practices](https://docs.docker.com/language/rust/)
- [Setu Project Documentation](../README.md)

## Support

For issues:
1. Check the Common Issues section in this document
2. Check project Issues on GitHub
3. Contact the development team
