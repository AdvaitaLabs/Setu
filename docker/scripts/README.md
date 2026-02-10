# Setu Docker 部署脚本

本目录包含用于管理 Setu Docker 服务的脚本。

## 脚本列表

### build.sh
构建 Docker 镜像。

```bash
./scripts/build.sh
```

功能：
- 构建 Validator 和 Solver 镜像
- 自动标记 latest 和 git commit hash 版本
- 启用 BuildKit 加速构建

### start.sh
启动 Setu 服务。

```bash
# 启动单验证器 + 单 Solver
./scripts/start.sh

# 启动单验证器 + 多 Solver
./scripts/start.sh --multi-solver

# 启动多验证器（测试共识）
./scripts/start.sh --multi-validator
```

功能：
- 自动创建 .env 文件（如果不存在）
- 根据模式启动相应的服务
- 显示服务访问信息

### stop.sh
停止 Setu 服务。

```bash
# 停止服务（保留数据）
./scripts/stop.sh

# 停止服务并删除数据卷
./scripts/stop.sh --volumes

# 停止多验证器模式
./scripts/stop.sh --multi-validator
```

功能：
- 优雅停止所有服务
- 可选删除数据卷
- 清理孤立容器

### logs.sh
查看服务日志。

```bash
# 查看所有服务日志
./scripts/logs.sh

# 查看特定服务日志
./scripts/logs.sh validator
./scripts/logs.sh solver-1

# 查看最近 50 行日志（不跟随）
./scripts/logs.sh validator --tail 50 --no-follow

# 查看多验证器模式日志
./scripts/logs.sh --multi-validator
```

功能：
- 实时跟随日志输出
- 过滤特定服务
- 自定义显示行数

## 使用流程

### 首次部署

```bash
# 1. 构建镜像
cd docker
./scripts/build.sh

# 2. 启动服务
./scripts/start.sh

# 3. 查看日志
./scripts/logs.sh

# 4. 验证服务
curl http://localhost:8080/api/v1/health
```

### 日常操作

```bash
# 查看服务状态
docker-compose ps

# 重启服务
./scripts/stop.sh
./scripts/start.sh

# 更新镜像
./scripts/build.sh
./scripts/stop.sh
./scripts/start.sh

# 清理并重新开始
./scripts/stop.sh --volumes
./scripts/start.sh
```

## 环境变量

所有脚本都支持通过 `.env` 文件配置环境变量。参考 `.env.example` 创建自己的配置。

## 故障排查

如果脚本执行失败：

1. 检查 Docker 是否运行：`docker ps`
2. 检查端口是否被占用：`lsof -i :8080`
3. 查看详细日志：`./scripts/logs.sh`
4. 清理并重试：`./scripts/stop.sh --volumes && ./scripts/start.sh`

