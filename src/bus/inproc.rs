// Copyright 2015-2020 Capital One Services, LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::errors;
use crate::{Invocation, InvocationResponse, Result};
use crossbeam::{Receiver, Sender};
use std::{collections::HashMap, sync::RwLock};

pub(crate) struct InprocBus {
    subscriptions: RwLock<HashMap<String, (Sender<Invocation>, Receiver<InvocationResponse>)>>,
}

impl InprocBus {
    pub fn new() -> Self {
        info!("Initialized Message Bus (internal)");
        InprocBus {
            subscriptions: RwLock::new(HashMap::new()),
        }
    }

    pub fn disconnect(&self) {
        // No-op
    }

    pub fn subscribe(
        &self,
        subject: &str,
        sender: crossbeam::Sender<Invocation>,
        receiver: crossbeam::Receiver<InvocationResponse>,
    ) -> Result<()> {
        self.subscriptions
            .write()
            .unwrap()
            .insert(subject.to_string(), (sender, receiver));
        Ok(())
    }

    pub fn invoke(&self, subject: &str, inv: Invocation) -> Result<InvocationResponse> {
        match self.subscriptions.read().unwrap().get(subject) {
            Some(s) => {
                s.0.send(inv).unwrap();
                let r = s.1.recv().unwrap();
                Ok(r)
            }
            None => Err(errors::new(errors::ErrorKind::MiscHost(format!(
                "Attempted bus call for {} with no subscribers",
                subject
            )))),
        }
    }

    pub fn unsubscribe(&self, subject: &str) -> Result<()> {
        self.subscriptions
            .write()
            .unwrap()
            .remove(&subject.to_string());
        Ok(())
    }
}
