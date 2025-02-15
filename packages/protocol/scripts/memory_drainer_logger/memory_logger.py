import docker
import time
from datetime import datetime
import logging
import sys
import os

# Configure logging to current directory
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(message)s',
    handlers=[
        logging.FileHandler('docker_memory.log'),
        logging.StreamHandler(sys.stdout)
    ]
)

def get_container_memory_usage(container_name):
    try:
        client = docker.from_env()  # Fixed: Correct way to initialize Docker client
        container = client.containers.get(container_name)
        stats = container.stats(stream=False)

        memory_usage = stats['memory_stats']['usage']
        memory_limit = stats['memory_stats']['limit']
        memory_percent = (memory_usage / memory_limit) * 100

        # Convert to MB
        memory_usage_mb = memory_usage / (1024 * 1024)
        memory_limit_mb = memory_limit / (1024 * 1024)

        return {
            'usage_mb': round(memory_usage_mb, 2),
            'limit_mb': round(memory_limit_mb, 2),
            'percent': round(memory_percent, 2)
        }
    except Exception as e:
        logging.error(f"Error getting container stats: {str(e)}")
        return None

def monitor_memory(container_name, interval=60):
    logging.info(f"Starting memory monitoring for container: {container_name}")
    logging.info(f"Logging interval: {interval} seconds")

    # Run in background
    if os.fork() != 0:
        return

    # Detach from terminal
    os.setsid()

    # Close file descriptors
    os.close(0)
    os.close(1)
    os.close(2)

    while True:
        try:
            stats = get_container_memory_usage(container_name)
            if stats:
                logging.info(
                    f"Memory Usage: {stats['usage_mb']}MB / {stats['limit_mb']}MB ({stats['percent']}%)"
                )
        except Exception as e:
            logging.error(f"Error: {str(e)}")
        time.sleep(interval)

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python memory_monitor.py <container_name> [interval_seconds]")
        sys.exit(1)

    container_name = sys.argv[1]
    interval = int(sys.argv[2]) if len(sys.argv) > 2 else 60

    monitor_memory(container_name, interval)
    print("Monitoring process started in background. Check docker_memory.log for output.")