use std::io::Error;
use std::net::TcpStream;
use std::process::id;

use tera::Context as TeraContext;

use crate::build_platform::Image;
use crate::cloud_provider::environment::Environment;
use crate::cloud_provider::kubernetes::Kubernetes;
use crate::cloud_provider::DeploymentTarget;
use crate::cmd::utilities::CmdError;
use crate::models::{Context, ProgressScope};
use crate::transaction::CommitError;

pub trait Service {
    fn context(&self) -> &Context;
    fn service_type(&self) -> ServiceType;
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn action(&self) -> &Action;
    fn private_port(&self) -> Option<u16>;
    fn total_cpus(&self) -> String;
    fn total_ram_in_mib(&self) -> u32;
    fn total_instances(&self) -> u16;
    fn is_listening(&self, ip: &str) -> bool {
        let private_port = match self.private_port() {
            Some(private_port) => private_port,
            _ => return false,
        };

        match TcpStream::connect(format!("{}:{}", ip, private_port)) {
            Ok(_) => true,
            Err(_) => false,
        }
    }

    fn is_valid(&self) -> Result<(), ServiceError> {
        let binaries = ["kubectl", "helm", "terraform", "aws-iam-authenticator"];

        for binary in binaries.iter() {
            if !crate::cmd::utilities::does_binary_exist(binary) {
                let err = format!("{} binary not found", binary);
                return Err(ServiceError::Unexpected(err));
            }
        }

        // TODO check lib directories available

        Ok(())
    }

    fn default_tera_context(
        &self,
        kubernetes: &dyn Kubernetes,
        environment: &Environment,
    ) -> TeraContext {
        let mut context = TeraContext::new();

        context.insert("id", self.id());
        context.insert("owner_id", environment.owner_id.as_str());
        context.insert("project_id", environment.project_id.as_str());
        context.insert("organization_id", environment.organization_id.as_str());
        context.insert("environment_id", environment.id.as_str());
        context.insert("region", kubernetes.region());
        context.insert("name", self.name());
        context.insert("namespace", environment.namespace());
        context.insert("cluster_name", kubernetes.name());
        context.insert("total_cpus", &self.total_cpus());
        context.insert("total_ram_in_mib", &self.total_ram_in_mib());
        context.insert("total_instances", &self.total_instances());

        context.insert("is_private_port", &self.private_port().is_some());
        if self.private_port().is_some() {
            context.insert("private_port", &self.private_port().unwrap());
        }

        context.insert("version", self.version());

        context
    }

    fn progress_scope(&self) -> ProgressScope {
        let id = self.id().to_string();

        match self.service_type() {
            ServiceType::Application => ProgressScope::Application { id },
            ServiceType::ExternalService => ProgressScope::ExternalService { id },
            ServiceType::Database(_) => ProgressScope::Database { id },
            ServiceType::Router => ProgressScope::Router { id },
        }
    }
}

pub trait StatelessService: Service + Create + Pause + Delete {
    fn exec_action(&self, deployment_target: &DeploymentTarget) -> Result<(), ServiceError> {
        match self.action() {
            crate::cloud_provider::service::Action::Create => self.on_create(deployment_target),
            crate::cloud_provider::service::Action::Delete => self.on_delete(deployment_target),
            crate::cloud_provider::service::Action::Pause => self.on_pause(deployment_target),
            crate::cloud_provider::service::Action::Nothing => Ok(()),
        }
    }
}

pub trait StatefulService:
    Service + Create + Pause + Delete + Backup + Clone + Upgrade + Downgrade
{
    fn exec_action(&self, deployment_target: &DeploymentTarget) -> Result<(), ServiceError> {
        match self.action() {
            crate::cloud_provider::service::Action::Create => self.on_create(deployment_target),
            crate::cloud_provider::service::Action::Delete => self.on_delete(deployment_target),
            crate::cloud_provider::service::Action::Pause => self.on_pause(deployment_target),
            crate::cloud_provider::service::Action::Nothing => Ok(()),
        }
    }
}

pub trait Application: StatelessService {
    fn image(&self) -> &Image;
    fn set_image(&mut self, image: Image);
}

pub trait ExternalService: Application {}

pub trait Router: StatelessService {
    fn check_domains(&self) -> Result<(), ServiceError>;
}

pub trait Database: StatefulService {}

pub trait Create {
    fn on_create(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
    fn on_create_check(&self) -> Result<(), ServiceError>;
    fn on_create_error(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
}

pub trait Pause {
    fn on_pause(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
    fn on_pause_check(&self) -> Result<(), ServiceError>;
    fn on_pause_error(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
}

pub trait Delete {
    fn on_delete(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
    fn on_delete_check(&self) -> Result<(), ServiceError>;
    fn on_delete_error(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
}

pub trait Backup {
    fn on_backup(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
    fn on_backup_check(&self) -> Result<(), ServiceError>;
    fn on_backup_error(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
    fn on_restore(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
    fn on_restore_check(&self) -> Result<(), ServiceError>;
    fn on_restore_error(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
}

pub trait Clone {
    fn on_clone(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
    fn on_clone_check(&self) -> Result<(), ServiceError>;
    fn on_clone_error(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
}

pub trait Upgrade {
    fn on_upgrade(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
    fn on_upgrade_check(&self) -> Result<(), ServiceError>;
    fn on_upgrade_error(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
}

pub trait Downgrade {
    fn on_downgrade(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
    fn on_downgrade_check(&self) -> Result<(), ServiceError>;
    fn on_downgrade_error(&self, target: &DeploymentTarget) -> Result<(), ServiceError>;
}

#[derive(Clone, Eq, PartialEq, Hash)]
pub enum Action {
    Create,
    Pause,
    Delete,
    Nothing,
}

#[derive(Eq, PartialEq)]
pub struct DatabaseOptions {
    pub login: String,
    pub password: String,
    pub host: String,
    pub port: u16,
    pub disk_size_in_gib: u32,
    pub database_disk_type: String,
}

#[derive(Eq, PartialEq)]
pub enum DatabaseType<'a> {
    PostgreSQL(&'a DatabaseOptions),
    MongoDB(&'a DatabaseOptions),
    MySQL(&'a DatabaseOptions),
}

#[derive(Eq, PartialEq)]
pub enum ServiceType<'a> {
    Application,
    ExternalService,
    Database(DatabaseType<'a>),
    Router,
}

impl<'a> ServiceType<'a> {
    pub fn name(&self) -> &str {
        match self {
            ServiceType::Application => "Application",
            ServiceType::ExternalService => "ExternalService",
            ServiceType::Database(_) => "Database",
            ServiceType::Router => "Router",
        }
    }
}

#[derive(Debug)]
pub enum ServiceError {
    OnCreateFailed,
    CheckFailed,
    Cmd(CmdError),
    Io(Error),
    NotEnoughResources(String),
    Unexpected(String),
}

impl From<std::io::Error> for ServiceError {
    fn from(err: Error) -> Self {
        ServiceError::Io(err)
    }
}

impl From<CmdError> for ServiceError {
    fn from(err: CmdError) -> Self {
        ServiceError::Cmd(err)
    }
}

impl From<CommitError> for Option<ServiceError> {
    fn from(err: CommitError) -> Self {
        return match err {
            CommitError::DeleteEnvironment(e)
            | CommitError::PauseEnvironment(e)
            | CommitError::DeployEnvironment(e)
            | CommitError::DeleteKubernetes(e)
            | CommitError::CreateKubernetes(e) => Option::from(e),
            CommitError::NotValidService(e) => Option::Some(e),
            _ => None,
        };
    }
}
