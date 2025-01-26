import requests
import time
import logging
from datetime import datetime
import os
from pathlib import Path

# Setup logging
log_dir = Path('logs')
log_dir.mkdir(exist_ok=True)

logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(levelname)s - %(message)s',
    handlers=[
        logging.FileHandler(log_dir / 'log_drain.log'),
        logging.StreamHandler()
    ]
)

def fetch_logs():
    try:
        response = requests.get('http://localhost:32005')

        # Create a timestamp for the filename
        timestamp = datetime.now().strftime('%Y%m%d_%H%M%S')
        output_file = log_dir / f'drain_{timestamp}.log'

        # Save the response content
        with open(output_file, 'w') as f:
            f.write(response.text)

        logging.info(f'Successfully fetched and saved logs to {output_file}')

    except requests.RequestException as e:
        logging.error(f'Failed to fetch logs: {str(e)}')
    except IOError as e:
        logging.error(f'Failed to save logs: {str(e)}')

def main():
    logging.info('Starting log drain service')

    # Run indefinitely
    while True:
        fetch_logs()
        # Sleep for 1 hour (3600 seconds)
        time.sleep(3600)

if __name__ == '__main__':
    main()