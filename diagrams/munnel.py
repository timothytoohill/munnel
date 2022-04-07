from diagrams import Cluster, Diagram
from diagrams.aws.compute import EC2
from diagrams.aws.database import RDS
from diagrams.aws.network import ELB
from diagrams.generic.os import Windows 
from diagrams.generic.network import Firewall
from diagrams.onprem.client import Client
from diagrams.onprem.compute import Server

with Diagram("Munnel Connectivity", show=False):
    with Cluster("Clients"):
        clients = [
            Client("Browser"),
            Client("VNC Client"),
            Client("Database Client"),
            Client("Any other\nTCP client")
        ]
    with Cluster("Servers"):
        servers = [
            Server("Munnel Agent On\nDatabase Server"),
            Server("Munnel Agent On\nVNC Server"),
            Server("Munnel Agent On\nWeb Server"),
            Server("Munnel Agent On\nany other server\naccepting TCP\nconnections.")
        ]
        with Cluster(""):
            firewall = Firewall("Servers running\nMunnel Agents\ncan be behind\na firewall")

    with Cluster(""):
        munnel_server = Server("Munnel Server")

    servers >> firewall >> munnel_server << clients

