use std::collections::HashSet;

use bson::oid::ObjectId;

use crate::{
    db::{Notification, User, UserID, OID},
    parsing::types::Class,
};

pub enum NotificationEvent {
    ClassDeleted {
        class: Class,
        affected_users: HashSet<UserID>,
    },
    Scheduled {
        class: Class,
        user_id: UserID,
    },
}
pub enum UpdateEvent {
    ClassRemoved {
        class: OID<Class>,
    },
    ClassAdded {
        class: OID<Class>,
    },

    /// User constrains were changed or created in some way
    UserUpdate {
        user: OID<User>,
    },
}
pub type NotificationEvents = smallvec::SmallVec<[NotificationEvent; 32]>;
pub type UpdateEvents = smallvec::SmallVec<[UpdateEvent; 32]>;

pub mod manager;

pub mod propagator {
    use std::convert::Infallible;

    use bson::doc;
    use chrono::Utc;
    use futures::StreamExt;
    use mongodb::Collection;
    use serde::Deserialize;
    use slog::Logger;
    use smallvec::SmallVec;

    use crate::{
        channels,
        db::{Model, Notification},
        parsing::types::Class,
    };

    use super::{NotificationEvent, NotificationEvents};

    #[derive(Debug, Deserialize)]
    pub struct Config {
        pub poll_interval: std::time::Duration,
    }

    pub struct Propagator {
        notifications: Collection<Notification>,
        classes: Collection<Class>,
        config: &'static Config,
        logger: Logger,
    }

    impl Propagator {
        pub fn new(db: &mongodb::Database, config: &'static Config, logger: &Logger) -> Self {
            Self {
                notifications: db.collection(&Notification::COLLECTION_NAME),
                classes: db.collection(Class::COLLECTION_NAME),
                logger: logger.new(slog::o!("subsystem" => "propagator")),
                config,
            }
        }

        async fn try_find_new(&self) -> eyre::Result<NotificationEvents> {
            let query = doc! {"fire_date": {"$lte": bson::DateTime::from_chrono(Utc::now())}};
            // notification that should be fired now
            let mut notifications = self.notifications.find(query.clone()).await?;

            let mut result = SmallVec::new();
            while let Some(notification) = notifications.next().await {
                let notification = notification?;

                let class = self
                    .classes
                    .find_one(doc! {"_id": &notification.related_class})
                    .await?;

                match class {
                    Some(class) => result.push(NotificationEvent::Scheduled {
                        class,
                        user_id: notification.related_user_id,
                    }),
                    None => {
                        // safe to skip because class might be cancelled
                        slog::warn!(self.logger, "propagator.error"; "desc" => "notification's related class wasn't found");
                    }
                }
            }

            self.notifications.delete_many(query).await?;

            Ok(result)
        }

        pub fn work(
            self,
            tx: impl channels::Tx<NotificationEvents>,
        ) -> tokio::task::JoinHandle<eyre::Result<Infallible>> {
            let mut interval = tokio::time::interval(self.config.poll_interval.clone());

            let fut = async move {
                loop {
                    interval.tick().await;
                    let results = self.try_find_new().await?;

                    match results.is_empty() {
                        true => {
                            slog::info!(self.logger, "no_new"; );
                        }
                        false => {
                            slog::info!(self.logger, "got_new_notifications_fired"; );
                            tx.send(results).await?;
                        }
                    }
                }
            };

            tokio::task::spawn(fut)
        }
    }
}
