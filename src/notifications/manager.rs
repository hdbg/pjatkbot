use std::{collections::HashSet, convert::Infallible, pin::Pin};

use bson::{doc, oid::ObjectId};
use chrono::{TimeDelta, Utc};
use eyre::OptionExt;
use futures::{Sink, Stream, StreamExt};
use mongodb::{Collection, Database};
use serde::Deserialize;
use slog::Logger;
use smallvec::smallvec;

use crate::{
    channels,
    db::{Model, Notification, NotificationConstraint, OIDCollection, User, UserID, OID},
    parsing::types::Class,
};

use super::{NotificationEvent, NotificationEvents, UpdateEvent, UpdateEvents};

#[derive(Debug, Deserialize)]
pub struct Config {
    full_resync_interval: std::time::Duration,
}

pub struct NotificationManager {
    users: OIDCollection<User>,
    classes: OIDCollection<Class>,
    notifications: Collection<Notification>,

    logger: Logger,

    config: &'static Config,
}

impl NotificationManager {
    pub fn new(config: &'static Config, db: &Database, logger: &Logger) -> Self {
        Self {
            users: db.collection(User::COLLECTION_NAME),
            classes: db.collection(Class::COLLECTION_NAME),
            notifications: db.collection(Notification::COLLECTION_NAME),

            logger: logger.new(slog::o!("subsystem" => "notifications_manager")),

            config,
        }
    }

    async fn remove_old_notifications(&self) -> eyre::Result<()> {
        let query = doc! {"fire_date": {"$lt": bson::DateTime::from_chrono(Utc::now())}};
        self.notifications.delete_many(query).await?;
        Ok(())
    }

    async fn upsert_notification(&self, notification: Notification) -> eyre::Result<()> {
        let as_doc = mongodb::bson::to_document(&notification)?;
        self.notifications
            .find_one_and_replace(as_doc, notification)
            .await?;
        Ok(())
    }

    async fn handle_class_add(&self, class: OID<Class>) -> eyre::Result<()> {
        // usually class contrains 1 group, so it's reasoanble to write loop
        // instead of complex query

        // but we should track users in case if users has 2 groups equal to class groups
        let mut seen_users = HashSet::new();

        for current_group in class.data.groups.iter() {
            let mut affected_users = self
                .users
                .find(doc! {"groups": &current_group.code})
                .await?;

            while let Some(user) = affected_users.next().await {
                let user = user?;

                if seen_users.contains(&user.id) {
                    continue;
                }

                for constraint in user.data.constraints.iter() {
                    let notification_time =
                        class.data.range.start - TimeDelta::from_std(constraint.0.clone())?;

                    if notification_time < Utc::now() {
                        continue;
                    }

                    let notification = Notification {
                        related_user: user.id.clone(),
                        related_class: class.id.clone(),
                        fire_date: notification_time,
                        related_user_id: user.data.telegram_id,
                    };
                    slog::info!(self.logger, "handle_class_add.new_notification"; "notification" => ?notification);

                    self.upsert_notification(notification).await?;
                    seen_users.insert(user.id.clone());
                }
            }
        }
        slog::debug!(self.logger, "handle_class_add.finished");

        Ok(())
    }

    async fn handle_class_removal(&self, class: OID<Class>) -> eyre::Result<NotificationEvent> {
        let mut final_users_affected = HashSet::new();

        // again, usually classes have a few groups
        for class_group in class.data.groups.iter() {
            let mut users_in_this_group =
                self.users.find(doc! {"group": &class_group.code}).await?;

            while let Some(user) = users_in_this_group.next().await {
                let user = user?;
                final_users_affected.insert(user.data.telegram_id);
            }
        }

        slog::info!(self.logger, "handle_class_removal"; "class" => ?class);

        Ok(NotificationEvent::ClassDeleted {
            class: class.data,
            affected_users: final_users_affected,
        })
    }

    async fn full_resync(&self) -> eyre::Result<()> {
        let student_and_all_classes = [
            doc! {
                "$unwind": doc! {
                    "path": "$groups",
                    "preserveNullAndEmptyArrays": false
                }
            },
            doc! {
                "$lookup": doc! {
                    "as": "classes",
                    "from": "classes",
                    "foreignField": "groups",
                    "localField": "groups"
                }
            },
        ];

        let mut student_and_all_classes = self.users.aggregate(student_and_all_classes).await?;

        while let Some(student_and_classes) = student_and_all_classes.next().await {
            let Ok(mut student_and_classes) = student_and_classes else {
                slog::error!(self.logger, "full_resync.deser_error");
                continue;
            };
            let Some(Ok(classes)) = student_and_classes
                .remove("classes")
                .map(|bson_classes| mongodb::bson::from_bson::<Vec<OID<Class>>>(bson_classes))
            else {
                slog::error!(self.logger, "full_resync.deser_error"; "where" => "get of classes");
                continue;
            };

            let student: OID<bson::Document> = mongodb::bson::from_document(student_and_classes)?;
            let telegram_id: UserID = bson::from_bson(student.data.get("id").unwrap().clone())?;

            let constraints: Vec<NotificationConstraint> =
                mongodb::bson::from_bson(student.data.get("constraints").unwrap().clone())?;

            if constraints.is_empty() {
                continue;
            }

            for class in classes {
                if class.data.range.start < Utc::now() {
                    slog::warn!(self.logger, "full_resync.class_to_old"; );
                    continue;
                }

                for constraint in constraints.iter() {
                    let new_time =
                        class.data.range.start - TimeDelta::from_std(constraint.0.clone())?;

                    // notification would fire right-away
                    if new_time < Utc::now() {
                        continue;
                    }

                    let notification = Notification {
                        related_user: student.id.clone(),
                        related_class: class.id.clone(),
                        fire_date: new_time,
                        related_user_id: telegram_id,
                    };

                    let notification_doc = mongodb::bson::to_document(&notification)?;

                    // insert new class if not exists
                    if self
                        .notifications
                        .find_one(notification_doc.clone())
                        .await?
                        .is_none()
                    {
                        slog::info!(self.logger, "full_resync.added_new"; "notification" => ?notification_doc);
                        self.notifications.insert_one(notification).await?;
                    }
                }
            }
        }

        self.remove_old_notifications().await?;
        Ok(())
    }

    async fn handle_user_update(&self, user: &OID<User>) -> eyre::Result<()> {
        self.notifications
            .delete_many(doc! {"related_user": &user.id})
            .await?;

        for group in user.data.groups.iter() {
            // don't care about collisions here because notifications are upserted
            let mut affected_classes = self.classes.find(doc! {"groups": &group.code}).await?;

            while let Some(class) = affected_classes.next().await {
                let class = class?;
                for constraint in user.data.constraints.iter() {
                    let new_time =
                        class.data.range.start - TimeDelta::from_std(constraint.0.clone())?;

                    // notification would fire right-away
                    if new_time < Utc::now() {
                        continue;
                    }

                    let notification = Notification {
                        related_user: user.id.clone(),
                        related_class: class.id.clone(),
                        fire_date: new_time,
                        related_user_id: user.data.telegram_id,
                    };

                    self.upsert_notification(notification).await?;
                }
            }
        }

        Ok(())
    }

    async fn handle_message(&self, msg: UpdateEvent) -> eyre::Result<Option<NotificationEvent>> {
        match msg {
            UpdateEvent::ClassRemoved { class } => {
                return Ok(Some(self.handle_class_removal(class).await?));
            }
            UpdateEvent::UserUpdate { user } => {
                self.handle_user_update(&user).await?;
            }
            UpdateEvent::ClassAdded { class } => {
                self.handle_class_add(class).await?;
            }
        }

        Ok(None)
    }

    pub async fn work(
        self,
        rx: impl channels::Rx<UpdateEvents>,
        tx: impl channels::Tx<NotificationEvents>,
    ) -> eyre::Result<tokio::task::JoinHandle<eyre::Result<Infallible>>> {
        self.full_resync().await?;
        let fut = async move {
            let mut resync_interval =
                tokio::time::interval(self.config.full_resync_interval.clone());

            resync_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    // _ = resync_interval.tick() => {
                    //     match self.full_resync().await {
                    //         Ok(_) => {
                    //             slog::info!(self.logger, "loop.resync.ok");
                    //         },
                    //         Err(err) => {
                    //             slog::error!(self.logger, "loop.full_resync_error"; "err" => ?err);
                    //         }
                    //     }
                    // }
                    msg = rx.recv() => {
                        match msg {
                            Ok(msgs) => {
                                slog::debug!(self.logger, "received_messages");
                                for msg in msgs {
                                    let response = self.handle_message(msg).await;
                                    match response {
                                        Ok(Some(msg)) => {
                                            tx.send(smallvec![msg]).await?;
                                        }
                                        Ok(None) => {}
                                        Err(err) => {
                                            slog::error!(self.logger, "loop.channel_closed"; "err" => ?err);

                                        }
                                    }
                                }
                            },
                            Err(err) => {
                                slog::error!(self.logger, "loop.channel_closed"; "err" => ?err);

                            }
                        }
                    }

                }
            }
        };

        Ok(tokio::task::spawn(fut))
    }
}
